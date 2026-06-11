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
        "INSERT INTO soll.Node (id, title, description) VALUES ('REQ-AXO-XYZ', 'Title with ?', 'Does this fail? Yes or no?')"
    );
}

#[test]
fn expand_named_params_skips_question_mark_inside_string_literal() {
    let query = "SELECT * FROM Node WHERE comment = 'is it ?' AND id = ?";
    let params = serde_json::json!(["abc"]);
    let expanded = GraphStore::expand_named_params(query, &params).unwrap();
    assert_eq!(
        expanded,
        "SELECT * FROM Node WHERE comment = 'is it ?' AND id = 'abc'"
    );
}

#[test]
fn expand_named_params_handles_escaped_quote_inside_literal() {
    let query = "SELECT * FROM Node WHERE title = 'don''t worry ?' AND id = ?";
    let params = serde_json::json!(["abc"]);
    let expanded = GraphStore::expand_named_params(query, &params).unwrap();
    assert_eq!(
        expanded,
        "SELECT * FROM Node WHERE title = 'don''t worry ?' AND id = 'abc'"
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
