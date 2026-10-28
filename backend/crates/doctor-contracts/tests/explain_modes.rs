use schemars::schema_for;
use serde_json::json;

#[test]
fn request_modes_default_only_when_omitted_and_keep_sql_write_only() {
    use doctor_contracts::{ExplainMode, ExplainRequest};
    for input in [
        json!({"input_kind":"user_sql","connection_id":"conn","candidate_sql":"SELECT private_column FROM private_table"}),
        json!({"input_kind":"plan_sql_reference","connection_id":"conn","query_id":"query","discovery_generation":1,"plan_sql_ref":"ref"}),
    ] {
        let request: ExplainRequest = serde_json::from_value(input.clone()).unwrap();
        let mode = match &request {
            ExplainRequest::UserSql { mode, .. }
            | ExplainRequest::PlanSqlReference { mode, .. } => mode,
        };
        assert_eq!(mode, &ExplainMode::Costs);
        assert!(!format!("{request:?}").contains("private_"));
        assert!(
            serde_json::to_value(&request)
                .unwrap()
                .get("candidate_sql")
                .is_none()
        );
        for mode in ["logical", "verbose", "costs"] {
            let mut explicit = input.clone();
            explicit["mode"] = json!(mode);
            let request: ExplainRequest = serde_json::from_value(explicit).unwrap();
            assert_eq!(serde_json::to_value(&request).unwrap()["mode"], mode);
        }
        for invalid in [
            json!(null),
            json!("analyze"),
            json!("costs; SELECT 1"),
            json!(1),
        ] {
            let mut explicit = input.clone();
            explicit["mode"] = invalid;
            assert!(serde_json::from_value::<ExplainRequest>(explicit).is_err());
        }
    }
}

#[test]
fn ordinary_explain_modes_are_a_closed_non_execution_grammar() {
    use doctor_contracts::ExplainMode;
    for (wire, mode) in [
        ("logical", ExplainMode::Logical),
        ("verbose", ExplainMode::Verbose),
        ("costs", ExplainMode::Costs),
    ] {
        assert_eq!(
            serde_json::from_value::<ExplainMode>(json!(wire)).unwrap(),
            mode
        );
        assert_eq!(serde_json::to_value(&mode).unwrap(), json!(wire));
    }
    for invalid in [
        json!("analyze"),
        json!("COSTS"),
        json!("costs; SELECT 1"),
        json!(""),
        json!(null),
        json!(1),
        json!({"mode": "costs"}),
    ] {
        assert!(serde_json::from_value::<ExplainMode>(invalid).is_err());
    }
    let schema = serde_json::to_value(schema_for!(ExplainMode)).unwrap();
    assert_eq!(schema["enum"], json!(["logical", "verbose", "costs"]));
}
