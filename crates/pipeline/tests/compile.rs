use calimero_pipeline::{
    canonical_json, compile, SpecError, MAX_NAME_BYTES, MAX_SOURCE_BYTES, MAX_STEPS,
};
use proptest::prelude::*;

const HEAD: &str = "apiVersion: compute.calimero.network/v1\nkind: Pipeline\nname: p\nmodule: m\n";

fn doc(rest: &str) -> String {
    format!("{HEAD}{rest}")
}

fn errors(src: &str) -> Vec<SpecError> {
    compile(src).expect_err("expected compile errors")
}

fn assert_error(src: &str, code: &str, path: &str) {
    let errs = errors(src);
    assert!(
        errs.iter().any(|e| e.code == code && e.path == path),
        "wanted {code} at {path}, got {errs:#?}\nsource:\n{src}"
    );
}

#[test]
fn minimal_pipeline_compiles() {
    compile(&doc("steps:\n  a: { method: run }\n")).unwrap();
}

#[test]
fn each_structural_error_has_a_code_and_path() {
    let steps = |body: &str| doc(&format!("steps:\n{body}"));
    let cases: Vec<(&str, String, &str, &str)> = vec![
        ("missing steps", HEAD.to_string(), "MISSING_FIELD", "steps"),
        ("missing method", steps("  a: {}\n"), "MISSING_FIELD", "steps.a.method"),
        (
            "unknown top field",
            doc("bogus: 1\nsteps:\n  a: { method: r }\n"),
            "UNKNOWN_FIELD",
            "bogus",
        ),
        ("unknown step field", steps("  a: { method: r, bogus: 1 }\n"), "UNKNOWN_FIELD", "steps.a.bogus"),
        ("when", steps("  a: { method: r, when: \"x\" }\n"), "NOT_IN_V1", "steps.a.when"),
        ("runIf", steps("  a: { method: r, runIf: failure }\n"), "NOT_IN_V1", "steps.a.runIf"),
        ("uses", steps("  a: { method: r, uses: x }\n"), "NOT_IN_V1", "steps.a.uses"),
        (
            "top-level loops",
            doc("loops: []\nsteps:\n  a: { method: r }\n"),
            "NOT_IN_V1",
            "loops",
        ),
        (
            "variables",
            doc("variables: {}\nsteps:\n  a: { method: r }\n"),
            "NOT_IN_V1",
            "variables",
        ),
        ("steps not a map", doc("steps: [a]\n"), "INVALID_TYPE", "steps"),
        (
            "wrong scalar type",
            steps("  a: { method: r, forEach: inputs.x, maxItems: lots }\n"),
            "INVALID_TYPE",
            "steps.a.maxItems",
        ),
        (
            "bad allowFailures",
            steps("  a: { method: r, forEach: inputs.x, maxItems: 1, allowFailures: 150% }\n"),
            "INVALID_TYPE",
            "steps.a.allowFailures",
        ),
        (
            "bad api version",
            "apiVersion: v2\nkind: Pipeline\nname: p\nmodule: m\nsteps:\n  a: { method: r }\n".into(),
            "BAD_API_VERSION",
            "apiVersion",
        ),
        (
            "bad kind",
            "apiVersion: compute.calimero.network/v1\nkind: Job\nname: p\nmodule: m\nsteps:\n  a: { method: r }\n".into(),
            "BAD_KIND",
            "kind",
        ),
        (
            "uppercase pipeline name",
            "apiVersion: compute.calimero.network/v1\nkind: Pipeline\nname: Bad\nmodule: m\nsteps:\n  a: { method: r }\n".into(),
            "BAD_NAME",
            "name",
        ),
        ("digit-first step name", steps("  1a: { method: r }\n"), "BAD_NAME", "steps.1a"),
        ("underscore step name", steps("  a_b: { method: r }\n"), "BAD_NAME", "steps.a_b"),
        (
            "bad input name",
            doc("inputs:\n  Bad: { kind: files }\nsteps:\n  a: { method: r }\n"),
            "BAD_NAME",
            "inputs.Bad",
        ),
        (
            "bad param name",
            doc("params:\n  Bad: { type: number }\nsteps:\n  a: { method: r }\n"),
            "BAD_NAME",
            "params.Bad",
        ),
        ("reserved step name", steps("  inputs: { method: r }\n"), "RESERVED_NAME", "steps.inputs"),
        ("forEach not a ref", steps("  a: { method: r, forEach: nope, maxItems: 1 }\n"), "BAD_REF", "steps.a.forEach"),
        ("forEach over params", steps("  a: { method: r, forEach: params.x, maxItems: 1 }\n"), "BAD_REF", "steps.a.forEach"),
        ("forEach over item", steps("  a: { method: r, forEach: item, maxItems: 1 }\n"), "BAD_REF", "steps.a.forEach"),
        ("unknown input", steps("  a: { method: r, inputs: [inputs.x] }\n"), "UNRESOLVED_REF", "steps.a.inputs[0]"),
        ("unknown param", steps("  a: { method: r, args: { k: params.x } }\n"), "UNRESOLVED_REF", "steps.a.args.k"),
        ("unknown step ref", steps("  a: { method: r, args: { k: ghost.result } }\n"), "UNRESOLVED_REF", "steps.a.args.k"),
        ("nested args ref", steps("  a: { method: r, args: { k: [ { z: params.x } ] } }\n"), "UNRESOLVED_REF", "steps.a.args.k[0].z"),
        ("input ref in args", doc("inputs:\n  x: { kind: files }\nsteps:\n  a: { method: r, args: { k: inputs.x } }\n"), "BAD_REF", "steps.a.args.k"),
        ("outputs ref in args", steps("  b: { method: r }\n  a: { method: r, args: { k: b.outputs } }\n"), "BAD_REF", "steps.a.args.k"),
        ("result ref in inputs", steps("  b: { method: r }\n  a: { method: r, inputs: [b.result] }\n"), "BAD_REF", "steps.a.inputs[0]"),
        ("item outside forEach", steps("  a: { method: r, inputs: [item] }\n"), "ITEM_OUTSIDE_FOREACH", "steps.a.inputs[0]"),
        ("item in args outside forEach", steps("  a: { method: r, args: { k: item } }\n"), "ITEM_OUTSIDE_FOREACH", "steps.a.args.k"),
        ("unknown need", steps("  a: { method: r, needs: [ghost] }\n"), "UNKNOWN_NEED", "steps.a.needs"),
        ("missing maxItems", doc("inputs:\n  x: { kind: files }\nsteps:\n  a: { method: r, forEach: inputs.x }\n"), "MISSING_MAX_ITEMS", "steps.a.maxItems"),
        ("maxItems zero", doc("inputs:\n  x: { kind: files }\nsteps:\n  a: { method: r, forEach: inputs.x, maxItems: 0 }\n"), "BAD_MAX_ITEMS", "steps.a.maxItems"),
        ("maxItems too big", doc("inputs:\n  x: { kind: files }\nsteps:\n  a: { method: r, forEach: inputs.x, maxItems: 10001 }\n"), "BAD_MAX_ITEMS", "steps.a.maxItems"),
        ("input maxItems too big", doc("inputs:\n  x: { kind: files, maxItems: 10001 }\nsteps:\n  a: { method: r }\n"), "BAD_MAX_ITEMS", "inputs.x.maxItems"),
        ("batch zero", doc("inputs:\n  x: { kind: files }\nsteps:\n  a: { method: r, forEach: inputs.x, maxItems: 1, batch: 0 }\n"), "BAD_BATCH", "steps.a.batch"),
        ("maxParallel zero", doc("inputs:\n  x: { kind: files }\nsteps:\n  a: { method: r, forEach: inputs.x, maxItems: 1, maxParallel: 0 }\n"), "BAD_MAX_PARALLEL", "steps.a.maxParallel"),
        ("maxItems without forEach", steps("  a: { method: r, maxItems: 3 }\n"), "FOREACH_REQUIRED", "steps.a.maxItems"),
        ("batch without forEach", steps("  a: { method: r, batch: 3 }\n"), "FOREACH_REQUIRED", "steps.a.batch"),
        ("allowFailures without forEach", steps("  a: { method: r, allowFailures: 5% }\n"), "ALLOW_FAILURES_NOT_FOREACH", "steps.a.allowFailures"),
        ("collect a non-forEach step", steps("  b: { method: r }\n  a: { method: r, collect: b.outputs }\n"), "COLLECT_TARGET", "steps.a.collect"),
        ("collect a ghost", steps("  a: { method: r, collect: ghost.outputs }\n"), "UNRESOLVED_REF", "steps.a.collect"),
        ("collect results not outputs", steps("  a: { method: r, collect: a.results }\n"), "BAD_REF", "steps.a.collect"),
        ("collect and forEach", doc("inputs:\n  x: { kind: files }\nsteps:\n  b: { method: r, forEach: inputs.x, maxItems: 1 }\n  a: { method: r, forEach: inputs.x, maxItems: 1, collect: b.outputs }\n"), "COLLECT_WITH_FOREACH", "steps.a.collect"),
        ("zero fuel limit", steps("  a: { method: r, limits: { fuel: 0 } }\n"), "BAD_LIMIT", "steps.a.limits.fuel"),
        ("unknown limit field", steps("  a: { method: r, limits: { gas: 1 } }\n"), "UNKNOWN_FIELD", "steps.a.limits.gas"),
        ("param default wrong type", doc("params:\n  t: { type: number, default: high }\nsteps:\n  a: { method: r }\n"), "BAD_PARAM", "params.t.default"),
        ("param default out of range", doc("params:\n  t: { type: number, default: 2, max: 1 }\nsteps:\n  a: { method: r }\n"), "BAD_PARAM", "params.t.default"),
        ("param min above max", doc("params:\n  t: { type: number, min: 3, max: 1 }\nsteps:\n  a: { method: r }\n"), "BAD_PARAM", "params.t.min"),
        ("param default not in enum", doc("params:\n  c: { type: string, default: GBP, enum: [EUR, USD] }\nsteps:\n  a: { method: r }\n"), "BAD_PARAM", "params.c.default"),
        ("source without input", doc("sources:\n  x: { connector: s3, uri: 's3://b/*' }\nsteps:\n  a: { method: r }\n"), "UNRESOLVED_REF", "sources.x"),
        ("publish to ghost sink", steps("  a: { method: r }\npublish:\n  - { from: a.outputs, to: sinks.ghost }\n"), "UNRESOLVED_REF", "publish[0].to"),
        ("publish to non-sink", steps("  a: { method: r }\npublish:\n  - { from: a.outputs, to: out }\n"), "BAD_REF", "publish[0].to"),
        ("publish from ghost", doc("sinks:\n  s: { connector: s3, uri: 's3://b/' }\nsteps:\n  a: { method: r }\npublish:\n  - { from: ghost.outputs, to: sinks.s }\n"), "UNRESOLVED_REF", "publish[0].from"),
        ("outputs ghost", steps("  a: { method: r }\noutputs: [ghost.outputs]\n"), "UNRESOLVED_REF", "outputs[0]"),
        ("duplicate yaml key", steps("  a: { method: r, method: s }\n"), "YAML_PARSE", ""),
        ("malformed yaml", "steps: [".into(), "YAML_PARSE", ""),
    ];
    for (name, src, code, path) in &cases {
        let errs = errors(src);
        assert!(
            errs.iter().any(|e| e.code == *code && e.path == *path),
            "{name}: wanted {code} at `{path}`, got {errs:#?}"
        );
    }
}

#[test]
fn oversized_source_is_refused_before_parsing() {
    let src = format!("# {}\n", "x".repeat(MAX_SOURCE_BYTES));
    assert_error(&src, "TOO_LARGE", "");
}

#[test]
fn too_many_steps_is_refused() {
    let steps: String = (0..=MAX_STEPS)
        .map(|i| format!("  s{i}: {{ method: r }}\n"))
        .collect();
    assert_error(&doc(&format!("steps:\n{steps}")), "TOO_MANY", "steps");
}

#[test]
fn name_length_is_bounded() {
    let long = "a".repeat(MAX_NAME_BYTES + 1);
    assert_error(
        &doc(&format!("steps:\n  {long}: {{ method: r }}\n")),
        "BAD_NAME",
        &format!("steps.{long}"),
    );
    let ok = "a".repeat(MAX_NAME_BYTES);
    compile(&doc(&format!("steps:\n  {ok}: {{ method: r }}\n"))).unwrap();
}

#[test]
fn args_expanded_by_aliases_are_bounded() {
    let big = "x".repeat(MAX_SOURCE_BYTES / 2 - 200);
    let src = doc(&format!(
        "steps:\n  a:\n    method: r\n    args:\n      p: &v \"{big}\"\n      q: *v\n      r: *v\n"
    ));
    assert_error(&src, "ARGS_TOO_LARGE", "steps.a.args");
}

#[test]
fn alias_bomb_is_refused() {
    let mut src = String::from("l0: &l0 [x,x,x,x,x,x,x,x]\n");
    for i in 1..6 {
        let prev = format!("*l{}", i - 1);
        src.push_str(&format!("l{i}: &l{i} [{}]\n", vec![prev; 8].join(",")));
    }
    let bomb = doc(&format!("{src}steps:\n  a: {{ method: r }}\n"));
    assert_error(&bomb, "YAML_PARSE", "");
}

#[test]
fn spec_example_compiles_once_later_fields_are_removed() {
    let full = include_str!("invoice-risk.yaml");
    let errs = errors(full);
    let mut got: Vec<_> = errs.iter().map(|e| (e.code, e.path.as_str())).collect();
    got.sort();
    assert_eq!(
        got,
        [
            ("NOT_IN_V1", "steps.escalate.when"),
            ("NOT_IN_V1", "steps.incident.runIf")
        ]
    );

    let v1: String = full
        .lines()
        .map(|l| {
            l.replace(", when: \"score.results.exists(r, r.flagged)\"", "")
                .replace(", runIf: failure", "")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let p = compile(&v1).unwrap_or_else(|e| panic!("{e:#?}"));
    assert_eq!(p.steps["score"].needs, ["extract"]);
    assert_eq!(p.steps["report"].needs, ["score"]);
    assert_eq!(p.steps["incident"].needs, ["report", "score"]);
}

#[test]
fn a_reference_implies_needs_and_item_is_valid_inside_for_each() {
    let src = doc(
        "inputs:\n  x: { kind: files }\nsteps:\n  a: { method: r, forEach: inputs.x, maxItems: 2, inputs: [item] }\n  b: { method: r, args: { k: a.results } }\n",
    );
    let p = compile(&src).unwrap();
    assert_eq!(p.steps["b"].needs, ["a"]);
}

#[test]
fn cycle_path_is_reported() {
    let src = doc("steps:\n  a: { method: r, needs: [b] }\n  b: { method: r, needs: [c] }\n  c: { method: r, needs: [a] }\n");
    let e = errors(&src)
        .into_iter()
        .find(|e| e.code == "CYCLE")
        .unwrap();
    assert_eq!(e.path, "steps.a");
    assert!(e.message.ends_with("a -> b -> c -> a"), "{}", e.message);
}

#[test]
fn self_reference_is_a_cycle() {
    assert_error(
        &doc("steps:\n  a: { method: r, args: { k: a.result } }\n"),
        "CYCLE",
        "steps.a",
    );
}

#[test]
fn canonical_json_ignores_source_spelling() {
    let block = doc("steps:\n  b: { method: r, needs: [a] }\n  a:\n    method: r\n    cache: true\n    args: { y: 1, x: 2 }\n");
    let reindented = block.replace("  ", "    ");
    let flow = "{steps: {b: {needs: [a], method: r}, a: {args: {x: 2, y: 1}, method: r}}, module: m, name: p, kind: Pipeline, apiVersion: compute.calimero.network/v1}";
    let want = canonical_json(&compile(&block).unwrap());
    assert_eq!(want, canonical_json(&compile(&reindented).unwrap()));
    assert_eq!(want, canonical_json(&compile(flow).unwrap()));
    let uncached = block.replace("cache: true", "cache: false");
    assert_ne!(want, canonical_json(&compile(&uncached).unwrap()));
}

#[test]
fn implied_needs_canonicalize_like_explicit_ones() {
    let explicit =
        doc("steps:\n  a: { method: r }\n  b: { method: r, needs: [a], args: { k: a.result } }\n");
    let implied = doc("steps:\n  a: { method: r }\n  b: { method: r, args: { k: a.result } }\n");
    assert_eq!(
        canonical_json(&compile(&explicit).unwrap()),
        canonical_json(&compile(&implied).unwrap())
    );
}

#[test]
fn canonical_json_is_stable_json_with_sorted_keys() {
    let p = compile(&doc("steps:\n  a: { method: r }\n")).unwrap();
    let bytes = canonical_json(&p);
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(
        text.starts_with(
            r#"{"apiVersion":"compute.calimero.network/v1","failFast":false,"inputs":{}"#
        ),
        "{text}"
    );
    assert_eq!(bytes, canonical_json(&p));
}

fn has_cycle(n: usize, edge: &[bool]) -> bool {
    let mut indeg: Vec<usize> = (0..n)
        .map(|i| (0..n).filter(|&j| edge[i * n + j]).count())
        .collect();
    let mut ready: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut seen = 0;
    while let Some(j) = ready.pop() {
        seen += 1;
        for i in (0..n).filter(|&i| edge[i * n + j]) {
            indeg[i] -= 1;
            if indeg[i] == 0 {
                ready.push(i);
            }
        }
    }
    seen < n
}

proptest! {
    #[test]
    fn compile_accepts_exactly_the_acyclic_shapes(
        (n, edge) in (1usize..8).prop_flat_map(|n| {
            prop::collection::vec(prop::bool::weighted(0.25), n * n).prop_map(move |e| (n, e))
        })
    ) {
        let mut steps = String::from("steps:\n");
        for i in 0..n {
            let needs: Vec<String> = (0..n).filter(|&j| edge[i * n + j]).map(|j| format!("s{j}")).collect();
            steps.push_str(&format!("  s{i}: {{ method: r, needs: [{}] }}\n", needs.join(", ")));
        }
        let result = compile(&doc(&steps));
        if has_cycle(n, &edge) {
            let errs = result.expect_err("a cycle must not compile");
            prop_assert!(errs.iter().any(|e| e.code == "CYCLE"), "{:?}", errs);
        } else {
            prop_assert!(result.is_ok(), "{:?}", result);
        }
    }
}
