use atlas::{
    config::Limits,
    domain::Error,
    query::predicate::{Comparison, Predicate, Scalar, Spatial, parse},
};

const SQUARE: &str = "[[[0,0],[1,0],[1,1],[0,1],[0,0]]]";

fn ok(json: &str) -> Predicate {
    parse(json, &Limits::default()).unwrap()
}

fn rejected(json: &str, limits: &Limits) -> bool {
    parse(json, limits) == Err(Error::InvalidFilter)
}

fn nested(levels: usize) -> String {
    let leaf = r#"{"op":"is_null","field":"a"}"#;
    format!(
        "{}{leaf}{}",
        r#"{"op":"not","arg":"#.repeat(levels),
        "}".repeat(levels)
    )
}

fn group(args: usize) -> String {
    let leaf = r#"{"op":"eq","field":"a","value":1}"#;
    format!(r#"{{"op":"or","args":[{}]}}"#, vec![leaf; args].join(","))
}

#[test]
fn parses_attribute_predicates_and_preserves_numbers() {
    for op in ["eq", "ne", "gt", "gte", "lt", "lte", "like", "ilike"] {
        ok(&format!(r#"{{"op":"{op}","field":"a","value":"x%"}}"#));
    }
    assert!(matches!(
        ok(r#"{"op":"gte","field":"poblacion","value":100000}"#),
        Predicate::Compare { op: Comparison::Gte, field, .. } if field == "poblacion"
    ));
    assert!(matches!(
        ok(r#"{"op":"is_not_null","field":"nombre"}"#),
        Predicate::Null { negated: true, .. }
    ));
    let Predicate::Compare {
        value: Scalar::Number(n),
        ..
    } = ok(r#"{"op":"eq","field":"a","value":123456789012345678901234.5}"#)
    else {
        panic!("number")
    };
    assert_eq!(n.to_string(), "123456789012345678901234.5");
    let Predicate::In { values, .. } = ok(r#"{"op":"in","field":"a","value":["x",2,false]}"#)
    else {
        panic!("in")
    };
    assert_eq!(values[0], Scalar::Text("x".into()));
    assert_eq!(values[2], Scalar::Bool(false));
}

#[test]
fn parses_logical_groups() {
    let Predicate::And(args) = ok(
        r#"{"op":"and","args":[{"op":"gte","field":"poblacion","value":100000},{"op":"is_not_null","field":"nombre"}]}"#,
    ) else {
        panic!("and")
    };
    assert_eq!(args.len(), 2);
    assert!(matches!(
        ok(r#"{"op":"not","arg":{"op":"or","args":[{"op":"is_null","field":"a"}]}}"#),
        Predicate::Not(inner) if matches!(&*inner, Predicate::Or(args) if args.len() == 1)
    ));
    assert!(matches!(
        ok(&format!(
            r#"{{"op":"not","arg":{{"op":"within","geometry":{{"type":"MultiPolygon","coordinates":[{SQUARE},{SQUARE}]}}}}}}"#
        )),
        Predicate::Not(inner) if matches!(&*inner, Predicate::Spatial { op: Spatial::Within, polygons } if polygons.len() == 2)
    ));
}

#[test]
fn rejects_malformed_keys_types_and_operators() {
    let limits = Limits::default();
    for invalid in [
        r#"{"op":"eq","field":"a","value":1,"op":"eq"}"#,
        r#"{"op":"eq","field":"a","field":"b"}"#,
        r#"{"op":"eq","field":"a","value":1,"extra":2}"#,
        r#"{"op":"eq","field":"a"}"#,
        r#"{"op":"between","field":"a","value":1}"#,
        r#"{"op":"EQ","field":"a","value":1}"#,
        r#"{"field":"a","value":1}"#,
        r#"{"op":"eq","field":"","value":1}"#,
        r#"{"op":"eq","field":1,"value":1}"#,
        r#"{"op":"eq","field":"a","value":null}"#,
        r#"{"op":"eq","field":"a","value":[1]}"#,
        r#"{"op":"eq","field":"a","value":{"x":1}}"#,
        r#"{"op":"eq","field":"a","value":1e999}"#,
        r#"{"op":"eq","field":"a","value":NaN}"#,
        r#"{"op":"is_null","field":"a","value":true}"#,
        r#"{"op":"in","field":"a","value":[]}"#,
        r#"{"op":"in","field":"a","value":[[1]]}"#,
        r#"{"op":"in","field":"a","value":1}"#,
        r#"{"op":"and","args":[]}"#,
        r#"{"op":"and","args":{"op":"is_null","field":"a"}}"#,
        r#"{"op":"or","args":[{"op":"is_null","field":"a"},{"op":"is_null","field":"a","x":1}]}"#,
        r#"{"op":"not","arg":"a"}"#,
        r#"{"op":"is_null","field":"a"} {}"#,
        r#"[{"op":"is_null","field":"a"}]"#,
        "true",
        "",
    ] {
        assert!(rejected(invalid, &limits), "{invalid}");
    }
}

#[test]
fn validates_selection_geometry_structure() {
    let limits = Limits::default();
    let spatial = |geometry: &str| format!(r#"{{"op":"intersects","geometry":{geometry}}}"#);
    ok(&spatial(&format!(
        r#"{{"type":"Polygon","coordinates":{SQUARE}}}"#
    )));
    for invalid in [
        r#"{"type":"Point","coordinates":[0,0]}"#.to_string(),
        r#"{"type":"Polygon","coordinates":[]}"#.into(),
        r#"{"type":"MultiPolygon","coordinates":[[]]}"#.into(),
        r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[0,0]]]}"#.into(),
        r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1]]]}"#.into(),
        r#"{"type":"Polygon","coordinates":[[[0,0,0],[1,0,0],[1,1,0],[0,0,0]]]}"#.into(),
        r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,true],[0,0]]]}"#.into(),
        format!(r#"{{"type":"Polygon","coordinates":{SQUARE},"crs":4326}}"#),
        format!(r#"{{"type":"Polygon","type":"Polygon","coordinates":{SQUARE}}}"#),
    ] {
        assert!(rejected(&spatial(&invalid), &limits), "{invalid}");
    }
}

#[test]
fn enforces_exact_budgets() {
    let mut limits = Limits::default();
    let depth = limits.filter_depth as usize;
    assert!(parse(&nested(depth - 1), &limits).is_ok());
    assert!(rejected(&nested(depth), &limits));
    let nodes = limits.filter_nodes as usize;
    assert!(parse(&group(nodes - 1), &limits).is_ok());
    assert!(rejected(&group(nodes), &limits));

    let values = vec!["1"; limits.in_values as usize].join(",");
    let membership = format!(r#"{{"op":"in","field":"a","value":[{values}]}}"#);
    assert!(parse(&membership, &limits).is_ok());
    assert!(rejected(&membership.replace("[1,", "[1,1,"), &limits));

    let polygon =
        format!(r#"{{"op":"within","geometry":{{"type":"Polygon","coordinates":{SQUARE}}}}}"#);
    limits.xy_pairs = 5;
    assert!(parse(&polygon, &limits).is_ok());
    limits.xy_pairs = 4;
    assert!(rejected(&polygon, &limits));

    limits.filter_bytes = membership.len() as u32;
    assert!(parse(&membership, &limits).is_ok());
    limits.filter_bytes -= 1;
    assert!(rejected(&membership, &limits));
}

#[test]
fn deep_inputs_fail_without_stack_overflow() {
    let limits = Limits {
        filter_bytes: u32::MAX,
        filter_depth: 128,
        filter_nodes: u32::MAX,
        ..Limits::default()
    };
    assert!(rejected(&nested(10_000), &limits));
    assert!(rejected(&"[".repeat(1_000_000), &limits));
    let arrays = format!(
        r#"{{"op":"in","field":"a","value":{}{}}}"#,
        "[".repeat(100_000),
        "]".repeat(100_000)
    );
    assert!(rejected(&arrays, &limits));
}
