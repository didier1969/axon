"""Tests for the SQL parser."""

from __future__ import annotations

import pytest

from axon.core.parsers.sql_lang import SqlParser


@pytest.fixture
def parser() -> SqlParser:
    return SqlParser()


SQL_FIXTURE = """\
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) UNIQUE
);

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id),
    total DECIMAL(10, 2)
);

CREATE VIEW active_users AS
SELECT * FROM users WHERE active = true;

CREATE FUNCTION get_user_orders(user_id INT)
RETURNS TABLE (order_id INT, total DECIMAL)
AS $$
    SELECT id, total FROM orders WHERE orders.user_id = user_id;
$$ LANGUAGE plpgsql;

CREATE PROCEDURE cleanup_old_orders()
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM orders WHERE created_at < NOW() - INTERVAL '1 year';
END;
$$;

ALTER TABLE users ADD COLUMN active BOOLEAN DEFAULT true;
DROP TABLE IF EXISTS temp_data;
"""


class TestSqlTables:
    def test_tables_extracted(self, parser: SqlParser) -> None:
        result = parser.parse(SQL_FIXTURE, "schema.sql")
        tables = [s for s in result.symbols if s.kind == "class"]
        names = {t.name for t in tables}
        assert "users" in names
        assert "orders" in names

    def test_table_kind_is_class(self, parser: SqlParser) -> None:
        result = parser.parse(SQL_FIXTURE, "schema.sql")
        tables = [s for s in result.symbols if s.name == "users"]
        assert tables[0].kind == "class"


class TestSqlViews:
    def test_view_extracted(self, parser: SqlParser) -> None:
        result = parser.parse(SQL_FIXTURE, "schema.sql")
        views = [s for s in result.symbols if s.name == "active_users"]
        assert len(views) == 1
        assert views[0].kind == "function"


class TestSqlFunctions:
    def test_function_extracted(self, parser: SqlParser) -> None:
        result = parser.parse(SQL_FIXTURE, "schema.sql")
        funcs = [s for s in result.symbols if s.name == "get_user_orders"]
        assert len(funcs) == 1
        assert funcs[0].kind == "function"

    def test_procedure_extracted(self, parser: SqlParser) -> None:
        result = parser.parse(SQL_FIXTURE, "schema.sql")
        procs = [s for s in result.symbols if s.name == "cleanup_old_orders"]
        assert len(procs) == 1
        assert procs[0].kind == "function"


class TestSqlCalls:
    def test_alter_extracted(self, parser: SqlParser) -> None:
        result = parser.parse(SQL_FIXTURE, "schema.sql")
        alter_calls = [c for c in result.calls if c.name.startswith("ALTER:")]
        assert any(c.name == "ALTER:users" for c in alter_calls)

    def test_drop_extracted(self, parser: SqlParser) -> None:
        result = parser.parse(SQL_FIXTURE, "schema.sql")
        drop_calls = [c for c in result.calls if c.name.startswith("DROP:")]
        assert any(c.name == "DROP:temp_data" for c in drop_calls)


class TestSqlEdgeCases:
    def test_empty_file(self, parser: SqlParser) -> None:
        result = parser.parse("", "empty.sql")
        assert result.symbols == []
        assert result.imports == []
        assert result.calls == []

    def test_comments_only(self, parser: SqlParser) -> None:
        result = parser.parse("-- just a comment\n/* block */\n", "comments.sql")
        assert result.symbols == []
