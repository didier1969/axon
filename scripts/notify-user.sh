#!/usr/bin/env bash
set -euo pipefail

message="${1:-Axon needs your attention.}"

play_with_canberra() {
  command -v canberra-gtk-play >/dev/null 2>&1 || return 1
  canberra-gtk-play -i dialog-warning -d "$message" >/dev/null 2>&1
}

play_with_paplay() {
  command -v paplay >/dev/null 2>&1 || return 1
  local sound=""
  for candidate in \
    /usr/share/sounds/freedesktop/stereo/dialog-warning.oga \
    /usr/share/sounds/freedesktop/stereo/complete.oga \
    /usr/share/sounds/freedesktop/stereo/bell.oga
  do
    if [ -f "$candidate" ]; then
      sound="$candidate"
      break
    fi
  done
  [ -n "$sound" ] || return 1
  paplay "$sound" >/dev/null 2>&1
}

play_with_aplay() {
  command -v aplay >/dev/null 2>&1 || return 1
  local sound=""
  for candidate in \
    /usr/share/sounds/alsa/Front_Center.wav \
    /usr/share/sounds/alsa/Noise.wav
  do
    if [ -f "$candidate" ]; then
      sound="$candidate"
      break
    fi
  done
  [ -n "$sound" ] || return 1
  aplay -q "$sound" >/dev/null 2>&1
}

play_terminal_bell() {
  printf '\a' >/dev/tty 2>/dev/null || printf '\a'
}

play_with_windows_powershell() {
  command -v powershell.exe >/dev/null 2>&1 || return 1
  powershell.exe -NoProfile -Command "[console]::beep(1046,250); Start-Sleep -Milliseconds 120; [console]::beep(1318,350)" >/dev/null 2>&1
}

show_notification() {
  command -v notify-send >/dev/null 2>&1 || return 0
  notify-send "Axon" "$message" >/dev/null 2>&1 || true
}

show_notification

play_with_canberra \
  || play_with_paplay \
  || play_with_aplay \
  || play_with_windows_powershell \
  || play_terminal_bell
