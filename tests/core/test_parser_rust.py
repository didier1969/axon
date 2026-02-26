"""Tests for the Rust language parser."""

from __future__ import annotations

import pytest

from axon.core.parsers.rust_lang import RustParser


@pytest.fixture
def parser() -> RustParser:
    return RustParser()


# ---------------------------------------------------------------------------
# Function extraction
# ---------------------------------------------------------------------------


class TestParseFunctions:
    CODE = """\
fn private_func(x: i32) -> i32 {
    x + 1
}

pub fn public_func(name: &str) -> String {
    format!("hello {}", name)
}
"""

    def test_private_function_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "lib.rs")
        funcs = [s for s in result.symbols if s.kind == "function"]
        names = {f.name for f in funcs}
        assert "private_func" in names

    def test_public_function_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "lib.rs")
        funcs = [s for s in result.symbols if s.kind == "function"]
        names = {f.name for f in funcs}
        assert "public_func" in names

    def test_public_function_exported(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "lib.rs")
        assert "public_func" in result.exports

    def test_private_function_not_exported(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "lib.rs")
        assert "private_func" not in result.exports

    def test_function_lines(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "lib.rs")
        func = [s for s in result.symbols if s.name == "private_func"][0]
        assert func.start_line == 1


# ---------------------------------------------------------------------------
# Struct extraction
# ---------------------------------------------------------------------------


class TestParseStruct:
    CODE = """\
pub struct User {
    pub name: String,
    age: u32,
}

struct InternalState {
    data: Vec<u8>,
}
"""

    def test_struct_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "models.rs")
        structs = [s for s in result.symbols if s.kind == "struct"]
        names = {s.name for s in structs}
        assert "User" in names

    def test_private_struct_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "models.rs")
        structs = [s for s in result.symbols if s.kind == "struct"]
        names = {s.name for s in structs}
        assert "InternalState" in names

    def test_pub_struct_exported(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "models.rs")
        assert "User" in result.exports

    def test_private_struct_not_exported(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "models.rs")
        assert "InternalState" not in result.exports


# ---------------------------------------------------------------------------
# Enum extraction
# ---------------------------------------------------------------------------


class TestParseEnum:
    CODE = """\
pub enum Status {
    Active,
    Inactive,
    Pending,
}
"""

    def test_enum_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "status.rs")
        enums = [s for s in result.symbols if s.kind == "enum"]
        assert len(enums) == 1
        assert enums[0].name == "Status"

    def test_pub_enum_exported(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "status.rs")
        assert "Status" in result.exports


# ---------------------------------------------------------------------------
# Trait extraction
# ---------------------------------------------------------------------------


class TestParseTrait:
    CODE = """\
pub trait Processor {
    fn process(&self) -> Result<(), Error>;
    fn name(&self) -> &str;
}
"""

    def test_trait_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "traits.rs")
        traits = [s for s in result.symbols if s.kind == "interface"]
        assert len(traits) == 1
        assert traits[0].name == "Processor"

    def test_pub_trait_exported(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "traits.rs")
        assert "Processor" in result.exports

    def test_trait_methods_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "traits.rs")
        methods = [s for s in result.symbols if s.kind == "method"]
        method_names = {m.name for m in methods}
        assert "process" in method_names
        assert "name" in method_names


# ---------------------------------------------------------------------------
# Impl block
# ---------------------------------------------------------------------------


class TestParseImpl:
    CODE = """\
struct MyStruct {
    value: i32,
}

impl MyStruct {
    pub fn new(x: i32) -> Self {
        MyStruct { value: x }
    }

    fn internal(&self) -> i32 {
        self.value
    }
}

impl Display for MyStruct {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}
"""

    def test_impl_methods_have_class_name(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "impl.rs")
        methods = [s for s in result.symbols if s.kind == "method"]
        new_method = [m for m in methods if m.name == "new"]
        assert len(new_method) >= 1
        assert new_method[0].class_name == "MyStruct"

    def test_impl_trait_heritage(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "impl.rs")
        assert ("MyStruct", "implements", "Display") in result.heritage

    def test_impl_methods_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "impl.rs")
        method_names = {s.name for s in result.symbols if s.kind == "method"}
        assert "new" in method_names
        assert "internal" in method_names


# ---------------------------------------------------------------------------
# Module extraction
# ---------------------------------------------------------------------------


class TestParseMod:
    CODE = """\
mod utils {
    pub fn helper() -> bool {
        true
    }
}
"""

    def test_mod_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "lib.rs")
        modules = [s for s in result.symbols if s.kind == "module"]
        assert len(modules) == 1
        assert modules[0].name == "utils"

    def test_mod_function_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "lib.rs")
        funcs = [s for s in result.symbols if s.kind == "function"]
        assert any(f.name == "helper" for f in funcs)


# ---------------------------------------------------------------------------
# Type alias extraction
# ---------------------------------------------------------------------------


class TestParseTypeAlias:
    CODE = """\
pub type Callback = Box<dyn Fn()>;
type Result<T> = std::result::Result<T, MyError>;
"""

    def test_type_alias_extracted(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "types.rs")
        aliases = [s for s in result.symbols if s.kind == "type_alias"]
        names = {a.name for a in aliases}
        assert "Callback" in names

    def test_pub_type_alias_exported(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "types.rs")
        assert "Callback" in result.exports


# ---------------------------------------------------------------------------
# Import (use) extraction
# ---------------------------------------------------------------------------


class TestParseUse:
    CODE = """\
use std::collections::HashMap;
use foo::{A, B};
use std::io;
"""

    def test_scoped_import(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "main.rs")
        modules = [i.module for i in result.imports]
        assert any("HashMap" in m for m in modules)

    def test_grouped_import_names(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "main.rs")
        foo_imp = [i for i in result.imports if i.module == "foo"]
        assert len(foo_imp) >= 1
        assert "A" in foo_imp[0].names
        assert "B" in foo_imp[0].names

    def test_simple_use(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "main.rs")
        modules = [i.module for i in result.imports]
        assert any("io" in m for m in modules)


# ---------------------------------------------------------------------------
# Call extraction
# ---------------------------------------------------------------------------


class TestParseCalls:
    CODE = """\
fn main() {
    let v = vec![1, 2, 3];
    println!("hello");
    obj.method_call(42);
    foo();
    HashMap::new();
}
"""

    def test_macro_call(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "main.rs")
        macro_calls = [c for c in result.calls if "!" in c.name]
        macro_names = {c.name for c in macro_calls}
        assert "vec!" in macro_names or "println!" in macro_names

    def test_method_call(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "main.rs")
        method_calls = [c for c in result.calls if c.name == "method_call"]
        assert len(method_calls) >= 1
        assert method_calls[0].receiver == "obj"

    def test_function_call(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "main.rs")
        foo_calls = [c for c in result.calls if c.name == "foo"]
        assert len(foo_calls) >= 1

    def test_scoped_call(self, parser: RustParser) -> None:
        result = parser.parse(self.CODE, "main.rs")
        new_calls = [c for c in result.calls if c.name == "new"]
        assert len(new_calls) >= 1


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


class TestEdgeCases:
    def test_empty_file(self, parser: RustParser) -> None:
        result = parser.parse("", "empty.rs")
        assert result.symbols == []
        assert result.imports == []
        assert result.calls == []
        assert result.heritage == []

    def test_syntax_error_does_not_crash(self, parser: RustParser) -> None:
        code = "fn broken(\n"
        result = parser.parse(code, "broken.rs")
        assert isinstance(result, type(result))

    def test_nested_impl(self, parser: RustParser) -> None:
        code = """\
struct Foo;
impl Foo {
    fn bar(&self) {}
}
"""
        result = parser.parse(code, "test.rs")
        methods = [s for s in result.symbols if s.kind == "method"]
        assert any(m.name == "bar" for m in methods)
        assert all(m.class_name == "Foo" for m in methods if m.name == "bar")
