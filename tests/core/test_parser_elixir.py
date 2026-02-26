"""Tests for the Elixir language parser."""

from __future__ import annotations

import pytest

from axon.core.parsers.elixir_lang import ElixirParser


@pytest.fixture
def parser() -> ElixirParser:
    return ElixirParser()


# ---------------------------------------------------------------------------
# Module extraction
# ---------------------------------------------------------------------------


class TestParseModule:
    CODE = """\
defmodule MyApp.Server do
  def hello, do: :world
end
"""

    def test_module_symbol_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "server.ex")
        modules = [s for s in result.symbols if s.kind == "module"]
        assert len(modules) == 1

    def test_module_name(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "server.ex")
        mod = [s for s in result.symbols if s.kind == "module"][0]
        assert mod.name == "MyApp.Server"

    def test_module_lines(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "server.ex")
        mod = [s for s in result.symbols if s.kind == "module"][0]
        assert mod.start_line == 1
        assert mod.end_line == 3


# ---------------------------------------------------------------------------
# Function extraction
# ---------------------------------------------------------------------------


class TestParseFunctions:
    CODE = """\
defmodule Calc do
  def add(a, b) do
    a + b
  end

  defp subtract(a, b) do
    a - b
  end
end
"""

    def test_public_function_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "calc.ex")
        funcs = [s for s in result.symbols if s.kind == "function"]
        names = {f.name for f in funcs}
        assert "add" in names

    def test_private_function_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "calc.ex")
        funcs = [s for s in result.symbols if s.kind == "function"]
        names = {f.name for f in funcs}
        assert "subtract" in names

    def test_public_function_exported(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "calc.ex")
        assert "add" in result.exports

    def test_private_function_not_exported(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "calc.ex")
        assert "subtract" not in result.exports

    def test_function_has_class_name(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "calc.ex")
        funcs = [s for s in result.symbols if s.kind == "function"]
        for f in funcs:
            assert f.class_name == "Calc"


# ---------------------------------------------------------------------------
# Macro extraction
# ---------------------------------------------------------------------------


class TestParseMacros:
    CODE = """\
defmodule MyMacros do
  defmacro my_macro(x) do
    quote do: unquote(x)
  end

  defmacrop private_macro(x) do
    x
  end
end
"""

    def test_macro_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "macros.ex")
        macros = [s for s in result.symbols if s.kind == "macro"]
        names = {m.name for m in macros}
        assert "my_macro" in names

    def test_private_macro_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "macros.ex")
        macros = [s for s in result.symbols if s.kind == "macro"]
        names = {m.name for m in macros}
        assert "private_macro" in names

    def test_public_macro_exported(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "macros.ex")
        assert "my_macro" in result.exports

    def test_private_macro_not_exported(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "macros.ex")
        assert "private_macro" not in result.exports


# ---------------------------------------------------------------------------
# Struct extraction
# ---------------------------------------------------------------------------


class TestParseStruct:
    CODE = """\
defmodule User do
  defstruct [:name, :email, :age]
end
"""

    def test_struct_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "user.ex")
        structs = [s for s in result.symbols if s.kind == "struct"]
        assert len(structs) == 1

    def test_struct_class_name(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "user.ex")
        struct = [s for s in result.symbols if s.kind == "struct"][0]
        assert struct.class_name == "User"


# ---------------------------------------------------------------------------
# Import directives
# ---------------------------------------------------------------------------


class TestParseImports:
    CODE = """\
defmodule MyApp do
  alias Foo.Bar
  alias Foo.Baz, as: B
  import Ecto.Query
  use Phoenix.Controller
  require Logger
end
"""

    def test_alias_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "app.ex")
        modules = [i.module for i in result.imports]
        assert "Foo.Bar" in modules

    def test_alias_with_as_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "app.ex")
        baz_imp = [i for i in result.imports if i.module == "Foo.Baz"]
        assert len(baz_imp) == 1
        assert baz_imp[0].alias == "B"

    def test_import_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "app.ex")
        modules = [i.module for i in result.imports]
        assert "Ecto.Query" in modules

    def test_use_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "app.ex")
        modules = [i.module for i in result.imports]
        assert "Phoenix.Controller" in modules

    def test_require_extracted(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "app.ex")
        modules = [i.module for i in result.imports]
        assert "Logger" in modules

    def test_use_creates_heritage(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "app.ex")
        assert ("MyApp", "uses", "Phoenix.Controller") in result.heritage


# ---------------------------------------------------------------------------
# Heritage
# ---------------------------------------------------------------------------


class TestParseHeritage:
    def test_use_genserver_heritage(self, parser: ElixirParser) -> None:
        code = """\
defmodule MyServer do
  use GenServer
end
"""
        result = parser.parse(code, "server.ex")
        assert ("MyServer", "uses", "GenServer") in result.heritage

    def test_behaviour_heritage(self, parser: ElixirParser) -> None:
        code = """\
defmodule MyWorker do
  @behaviour GenServer
end
"""
        result = parser.parse(code, "worker.ex")
        assert ("MyWorker", "implements", "GenServer") in result.heritage


# ---------------------------------------------------------------------------
# Call extraction
# ---------------------------------------------------------------------------


class TestParseCalls:
    CODE = """\
defmodule MyServer do
  use GenServer

  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts)
  end

  def handle_call(:ping, _from, state) do
    {:reply, :pong, state}
  end

  defp do_work(x) do
    Logger.info("working")
    process(x)
  end
end
"""

    def test_module_qualified_call(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "server.ex")
        calls = [c for c in result.calls if c.name == "start_link"]
        assert any(c.receiver == "GenServer" for c in calls)

    def test_local_call(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "server.ex")
        call_names = {c.name for c in result.calls}
        assert "process" in call_names

    def test_logger_call(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "server.ex")
        calls = [c for c in result.calls if c.name == "info"]
        assert any(c.receiver == "Logger" for c in calls)


# ---------------------------------------------------------------------------
# OTP decorators
# ---------------------------------------------------------------------------


class TestOTPDecorators:
    CODE = """\
defmodule MyServer do
  use GenServer

  @impl GenServer
  def handle_call(:ping, _from, state) do
    {:reply, :pong, state}
  end

  def init(opts) do
    {:ok, opts}
  end
end
"""

    def test_impl_decorator_captured(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "server.ex")
        handle_call = [s for s in result.symbols if s.name == "handle_call"]
        assert len(handle_call) >= 1
        assert "@impl" in handle_call[0].decorators

    def test_init_otp_entry_point_decorator(self, parser: ElixirParser) -> None:
        result = parser.parse(self.CODE, "server.ex")
        init_syms = [s for s in result.symbols if s.name == "init"]
        assert len(init_syms) >= 1
        assert "init" in init_syms[0].decorators


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


class TestEdgeCases:
    def test_empty_file(self, parser: ElixirParser) -> None:
        result = parser.parse("", "empty.ex")
        assert result.symbols == []
        assert result.imports == []
        assert result.calls == []
        assert result.heritage == []

    def test_syntax_error_does_not_crash(self, parser: ElixirParser) -> None:
        code = "defmodule Broken do\n  def foo(\n"
        result = parser.parse(code, "broken.ex")
        assert isinstance(result, type(result))

    def test_nested_modules(self, parser: ElixirParser) -> None:
        code = """\
defmodule Outer do
  defmodule Inner do
    def inner_func, do: :ok
  end
end
"""
        result = parser.parse(code, "nested.ex")
        module_names = {s.name for s in result.symbols if s.kind == "module"}
        assert "Outer" in module_names
        assert "Inner" in module_names
