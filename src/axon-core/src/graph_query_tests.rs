// REQ-AXO-091 — expand_named_params must consume one positional
// parameter per `?` in the ORIGINAL query, not per `?` in the
// partially-substituted result. The previous implementation used
// `expanded.find('?')` after each substitution and got fooled by
// literal `?` chars inside an already-substituted user string —
// producing malformed SQL when soll_manager created a Requirement
// whose title or description contained a question mark.

use super::*;

#[test]
fn expand_named_params_handles_question_mark_in_string_value() {
    let query = "INSERT INTO soll.Node (id, title, description) VALUES (?, ?, ?)";
    let params = serde_json::json!(["REQ-AXO-XYZ", "Title with ?", "Does this fail? Yes or no?"]);
    let expanded = GraphStore::expand_named_params(query, &params).unwrap();
    assert_eq!(
        expanded,
        "INSERT INTO soll.Node (id, title, description) VALUES ($axp$REQ-AXO-XYZ$axp$, $axp$Title with ?$axp$, $axp$Does this fail? Yes or no?$axp$)"
    );
}

#[test]
fn expand_named_params_dollar_quotes_apostrophes_backslashes_backticks() {
    // REQ-AXO-901995 — string values are dollar-quoted, so apostrophes,
    // backslashes and backticks pass through verbatim (no escaping). The old
    // single-quote escaping broke entrench_nuance's metadata UPDATE on such text.
    let query = "UPDATE soll.Node SET metadata = ? WHERE id = ?";
    let params = serde_json::json!([
        r#"{"nuances":[{"statement":"don't `panic` over a \\path\\ now"}]}"#,
        "REQ-NEX-001"
    ]);
    let expanded = GraphStore::expand_named_params(query, &params).unwrap();
    assert_eq!(
        expanded,
        "UPDATE soll.Node SET metadata = $axp${\"nuances\":[{\"statement\":\"don't `panic` over a \\\\path\\\\ now\"}]}$axp$ WHERE id = $axp$REQ-NEX-001$axp$"
    );
}

#[test]
fn expand_named_params_grows_dollar_tag_on_collision() {
    // If the value itself contains the default tag, the tag grows so the literal
    // stays well-formed.
    let query = "UPDATE soll.Node SET description = ? WHERE id = ?";
    let params = serde_json::json!(["mentions $axp$ literally", "REQ-AXO-1"]);
    let expanded = GraphStore::expand_named_params(query, &params).unwrap();
    assert!(
        expanded.contains("$axp1$mentions $axp$ literally$axp1$"),
        "tag must grow past a collision: {expanded}"
    );
}

#[test]
fn expand_named_params_skips_question_mark_inside_string_literal() {
    let query = "SELECT * FROM Node WHERE comment = 'is it ?' AND id = ?";
    let params = serde_json::json!(["abc"]);
    let expanded = GraphStore::expand_named_params(query, &params).unwrap();
    assert_eq!(
        expanded,
        "SELECT * FROM Node WHERE comment = 'is it ?' AND id = $axp$abc$axp$"
    );
}

#[test]
fn expand_named_params_handles_escaped_quote_inside_literal() {
    let query = "SELECT * FROM Node WHERE title = 'don''t worry ?' AND id = ?";
    let params = serde_json::json!(["abc"]);
    let expanded = GraphStore::expand_named_params(query, &params).unwrap();
    assert_eq!(
        expanded,
        "SELECT * FROM Node WHERE title = 'don''t worry ?' AND id = $axp$abc$axp$"
    );
}

#[test]
fn expand_named_params_rejects_too_few_positional_params() {
    let query = "INSERT INTO Node (a, b) VALUES (?, ?)";
    let params = serde_json::json!(["only_one"]);
    let err = GraphStore::expand_named_params(query, &params).unwrap_err();
    assert!(
        err.to_string().contains("Too few positional parameters"),
        "{err}"
    );
}

#[test]
fn expand_named_params_rejects_too_many_positional_params() {
    let query = "INSERT INTO Node (a) VALUES (?)";
    let params = serde_json::json!(["one", "extra"]);
    let err = GraphStore::expand_named_params(query, &params).unwrap_err();
    assert!(
        err.to_string().contains("Too many positional parameters"),
        "{err}"
    );
}
