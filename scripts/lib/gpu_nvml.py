#!/usr/bin/env python3
"""Shared NVML GPU telemetry helper (REQ-AXO-902085).

Replaces the `nvidia-smi` subprocess probes that were scattered across the
qualification / sensor scripts. Everything goes through NVML via ctypes so
telemetry stays in-process, fast, and free of CLI parsing.

Contract:
- `gpu_status(device_index=0) -> dict` NEVER raises. On any failure it returns
  ``{"available": False, "source": "nvml", "error": "<reason>"}``.
- On success it returns a canonical superset dict (see KEYS below) so every
  consumer can map to its own historical schema.
- `nvmlShutdown()` is always called in a `finally`.

Environment:
- ``AXON_NVML_LIBRARY_PATH``        explicit libnvidia-ml path (highest priority).
- ``AXON_GPU_TELEMETRY_DEVICE_INDEX`` default device index when caller passes none.

INTERDIT: clock-lock (`nvidia-smi -lgc`) — read-only telemetry only.
"""

from __future__ import annotations

import ctypes
import ctypes.util
import os
from typing import Any

NVML_TEMPERATURE_GPU = 0
_DEFAULT_LIBRARY = "/usr/lib/wsl/lib/libnvidia-ml.so.1"
_NVML_DEVICE_NAME_BUFFER_SIZE = 96
_NVML_SYSTEM_DRIVER_VERSION_BUFFER_SIZE = 96

# Canonical keys returned on success (superset consumed by all callers).
KEYS = (
    "available",
    "source",
    "library",
    "name",
    "driver_version",
    "memory_total_mb",
    "memory_used_mb",
    "memory_free_mb",
    "utilization_gpu",
    "utilization_memory",
    "temperature_c",
    "power_w",
    "power_limit_w",
)


class NvmlMemoryInfo(ctypes.Structure):
    _fields_ = [
        ("total", ctypes.c_ulonglong),
        ("free", ctypes.c_ulonglong),
        ("used", ctypes.c_ulonglong),
    ]


class NvmlUtilizationInfo(ctypes.Structure):
    _fields_ = [
        ("gpu", ctypes.c_uint),
        ("memory", ctypes.c_uint),
    ]


def _env_int(name: str) -> int | None:
    value = os.environ.get(name)
    if value is None or not value.strip():
        return None
    try:
        return int(value.strip())
    except ValueError:
        return None


def nvml_library_candidates() -> list[str]:
    """Ordered, de-duplicated list of libnvidia-ml candidates to dlopen."""
    configured = os.environ.get("AXON_NVML_LIBRARY_PATH", "").strip()
    candidates: list[str] = []
    if configured:
        candidates.append(configured)
    discovered = ctypes.util.find_library("nvidia-ml")
    if discovered:
        candidates.append(discovered)
    candidates.extend([_DEFAULT_LIBRARY, "libnvidia-ml.so.1"])
    return list(dict.fromkeys(candidates))


def _bind(library: ctypes.CDLL) -> dict[str, Any]:
    """Bind the NVML entry points we need. Optional ones may be absent."""
    fns: dict[str, Any] = {}

    nvml_init = library.nvmlInit_v2
    nvml_init.restype = ctypes.c_int
    fns["init"] = nvml_init

    nvml_shutdown = library.nvmlShutdown
    nvml_shutdown.restype = ctypes.c_int
    fns["shutdown"] = nvml_shutdown

    get_handle = library.nvmlDeviceGetHandleByIndex_v2
    get_handle.argtypes = [ctypes.c_uint, ctypes.POINTER(ctypes.c_void_p)]
    get_handle.restype = ctypes.c_int
    fns["get_handle"] = get_handle

    get_memory = library.nvmlDeviceGetMemoryInfo
    get_memory.argtypes = [ctypes.c_void_p, ctypes.POINTER(NvmlMemoryInfo)]
    get_memory.restype = ctypes.c_int
    fns["get_memory"] = get_memory

    get_utilization = library.nvmlDeviceGetUtilizationRates
    get_utilization.argtypes = [ctypes.c_void_p, ctypes.POINTER(NvmlUtilizationInfo)]
    get_utilization.restype = ctypes.c_int
    fns["get_utilization"] = get_utilization

    # --- optional metrics: bind best-effort, tolerate missing symbols ----------
    try:
        get_name = library.nvmlDeviceGetName
        get_name.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_uint]
        get_name.restype = ctypes.c_int
        fns["get_name"] = get_name
    except AttributeError:
        fns["get_name"] = None

    try:
        get_driver = library.nvmlSystemGetDriverVersion
        get_driver.argtypes = [ctypes.c_char_p, ctypes.c_uint]
        get_driver.restype = ctypes.c_int
        fns["get_driver"] = get_driver
    except AttributeError:
        fns["get_driver"] = None

    try:
        get_temperature = library.nvmlDeviceGetTemperature
        get_temperature.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint,
            ctypes.POINTER(ctypes.c_uint),
        ]
        get_temperature.restype = ctypes.c_int
        fns["get_temperature"] = get_temperature
    except AttributeError:
        fns["get_temperature"] = None

    try:
        get_power = library.nvmlDeviceGetPowerUsage
        get_power.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint)]
        get_power.restype = ctypes.c_int
        fns["get_power"] = get_power
    except AttributeError:
        fns["get_power"] = None

    try:
        get_power_limit = library.nvmlDeviceGetEnforcedPowerLimit
        get_power_limit.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint)]
        get_power_limit.restype = ctypes.c_int
        fns["get_power_limit"] = get_power_limit
    except AttributeError:
        fns["get_power_limit"] = None

    return fns


def _status_for_library(candidate: str, device_index: int) -> dict[str, Any]:
    library = ctypes.CDLL(candidate)
    fns = _bind(library)

    if fns["init"]() != 0:
        return {"available": False, "source": "nvml", "error": "nvml_init_failed"}
    try:
        device = ctypes.c_void_p()
        if fns["get_handle"](device_index, ctypes.byref(device)) != 0:
            return {
                "available": False,
                "source": "nvml",
                "error": "nvml_device_handle_failed",
            }

        memory = NvmlMemoryInfo()
        if fns["get_memory"](device, ctypes.byref(memory)) != 0:
            return {
                "available": False,
                "source": "nvml",
                "error": "nvml_memory_info_failed",
            }

        utilization = NvmlUtilizationInfo()
        util_ok = fns["get_utilization"](device, ctypes.byref(utilization)) == 0

        result: dict[str, Any] = {
            "available": True,
            "source": "nvml",
            "library": candidate,
            "name": None,
            "driver_version": None,
            "memory_total_mb": int(memory.total // (1024 * 1024)),
            "memory_used_mb": int(memory.used // (1024 * 1024)),
            "memory_free_mb": int(memory.free // (1024 * 1024)),
            "utilization_gpu": int(utilization.gpu) if util_ok else None,
            "utilization_memory": int(utilization.memory) if util_ok else None,
            "temperature_c": None,
            "power_w": None,
            "power_limit_w": None,
        }

        if fns["get_name"] is not None:
            name_buf = ctypes.create_string_buffer(_NVML_DEVICE_NAME_BUFFER_SIZE)
            if fns["get_name"](device, name_buf, _NVML_DEVICE_NAME_BUFFER_SIZE) == 0:
                result["name"] = name_buf.value.decode("utf-8", "replace") or None

        if fns["get_driver"] is not None:
            driver_buf = ctypes.create_string_buffer(
                _NVML_SYSTEM_DRIVER_VERSION_BUFFER_SIZE
            )
            if fns["get_driver"](
                driver_buf, _NVML_SYSTEM_DRIVER_VERSION_BUFFER_SIZE
            ) == 0:
                result["driver_version"] = (
                    driver_buf.value.decode("utf-8", "replace") or None
                )

        if fns["get_temperature"] is not None:
            temp = ctypes.c_uint()
            if fns["get_temperature"](
                device, NVML_TEMPERATURE_GPU, ctypes.byref(temp)
            ) == 0:
                result["temperature_c"] = int(temp.value)

        if fns["get_power"] is not None:
            power_mw = ctypes.c_uint()
            if fns["get_power"](device, ctypes.byref(power_mw)) == 0:
                result["power_w"] = round(power_mw.value / 1000.0, 3)

        if fns["get_power_limit"] is not None:
            limit_mw = ctypes.c_uint()
            if fns["get_power_limit"](device, ctypes.byref(limit_mw)) == 0:
                result["power_limit_w"] = round(limit_mw.value / 1000.0, 3)

        return result
    finally:
        try:
            fns["shutdown"]()
        except Exception:
            pass


def gpu_status(device_index: int | None = None) -> dict[str, Any]:
    """Return canonical GPU telemetry via NVML. Never raises.

    On success returns a dict with the :data:`KEYS` superset; on any failure
    returns ``{"available": False, "source": "nvml", "error": "<reason>"}``.
    """
    if device_index is None:
        device_index = _env_int("AXON_GPU_TELEMETRY_DEVICE_INDEX") or 0

    last_error = ""
    for candidate in nvml_library_candidates():
        try:
            status = _status_for_library(candidate, device_index)
        except Exception as exc:  # dlopen / missing symbol / segfault-guard
            last_error = type(exc).__name__
            continue
        if status.get("available"):
            return status
        last_error = status.get("error", last_error)
    return {
        "available": False,
        "source": "nvml",
        "error": last_error or "nvml_unavailable",
    }


if __name__ == "__main__":
    import json

    print(json.dumps(gpu_status(), indent=2))
