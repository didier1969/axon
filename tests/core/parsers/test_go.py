"""Tests for the Go language parser."""

from __future__ import annotations

import pytest

from axon.core.parsers.go_lang import GoParser


@pytest.fixture
def parser() -> GoParser:
    return GoParser()


GO_FIXTURE = """\
package main

import (
    "fmt"
    "os"
)

type User struct {
    Name string
    Age  int
}

type Stringer interface {
    String() string
}

func (u *User) String() string {
    return fmt.Sprintf("%s (%d)", u.Name, u.Age)
}

func main() {
    u := &User{Name: "Alice", Age: 30}
    fmt.Println(u.String())
}

func helper(x int) int {
    return x + 1
}
"""


class TestGoFunctions:
    def test_functions_extracted(self, parser: GoParser) -> None:
        result = parser.parse(GO_FIXTURE, "main.go")
        func_names = {s.name for s in result.symbols if s.kind == "function"}
        assert "main" in func_names
        assert "helper" in func_names

    def test_function_line_numbers(self, parser: GoParser) -> None:
        result = parser.parse(GO_FIXTURE, "main.go")
        main_fn = [s for s in result.symbols if s.name == "main" and s.kind == "function"][0]
        assert main_fn.start_line > 0


class TestGoStructs:
    def test_struct_extracted(self, parser: GoParser) -> None:
        result = parser.parse(GO_FIXTURE, "main.go")
        structs = [s for s in result.symbols if s.kind == "struct"]
        assert len(structs) == 1
        assert structs[0].name == "User"

    def test_struct_exported(self, parser: GoParser) -> None:
        result = parser.parse(GO_FIXTURE, "main.go")
        assert "User" in result.exports


class TestGoInterfaces:
    def test_interface_extracted(self, parser: GoParser) -> None:
        result = parser.parse(GO_FIXTURE, "main.go")
        interfaces = [s for s in result.symbols if s.kind == "interface"]
        assert len(interfaces) == 1
        assert interfaces[0].name == "Stringer"


class TestGoMethods:
    def test_method_extracted(self, parser: GoParser) -> None:
        result = parser.parse(GO_FIXTURE, "main.go")
        methods = [s for s in result.symbols if s.kind == "method"]
        assert len(methods) == 1
        assert methods[0].name == "String"
        assert methods[0].class_name == "User"


class TestGoImports:
    def test_imports_extracted(self, parser: GoParser) -> None:
        result = parser.parse(GO_FIXTURE, "main.go")
        modules = {i.module for i in result.imports}
        assert "fmt" in modules
        assert "os" in modules


class TestGoCalls:
    def test_calls_extracted(self, parser: GoParser) -> None:
        result = parser.parse(GO_FIXTURE, "main.go")
        call_names = {c.name for c in result.calls}
        assert "Println" in call_names or "Sprintf" in call_names


class TestGoEdgeCases:
    def test_empty_file(self, parser: GoParser) -> None:
        result = parser.parse("", "empty.go")
        assert result.symbols == []
        assert result.imports == []
        assert result.calls == []
