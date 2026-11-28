//! The schemas of the tools a main session offers in the advanced mode,
//! compared with their interface (`tool_interfaces.json`: names, types,
//! allowed values, defaults and required fields, without descriptions and
//! without Read's `pages`, which super does not build). Every description in
//! a schema is the placeholder of its slot.

use crate::agent_tools_tests::{core_tools, settings};
use crate::background_tests::model;
use crate::session::manager::SessionManager;
use crate::session_runner::AgentSession;
use crate::settings::Mode;
use serde_json::{Map, Value};
use std::sync::Arc;
use super_agent::placeholder::{tool_description, tool_parameter};
use super_ai::{Registry, ThinkingLevel};

fn interfaces() -> Map<String, Value> {
    match serde_json::from_str(include_str!("tool_interfaces.json")).unwrap() {
        Value::Object(map) => map,
        _ => unreachable!("the interfaces are an object"),
    }
}

/// Removes every parameter description from `schema`, checking on the way
/// that each is the placeholder of its parameter, named by its path.
fn without_descriptions(tool: &str, path: &str, schema: &mut Value) {
    let Value::Object(object) = schema else {
        return;
    };
    if let Some(description) = object.remove("description") {
        assert!(
            !path.is_empty(),
            "{tool}: the schema itself has a description"
        );
        assert_eq!(
            description,
            Value::String(tool_parameter(tool, path)),
            "{tool}: the description of {path}"
        );
    }
    if let Some(Value::Object(properties)) = object.get_mut("properties") {
        for (name, property) in properties.iter_mut() {
            let nested = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            without_descriptions(tool, &nested, property);
        }
    }
}

fn main_session() -> (tempfile::TempDir, Arc<AgentSession>) {
    let directory = tempfile::tempdir().unwrap();
    let manager =
        SessionManager::create(directory.path(), Some(directory.path().join("sessions"))).unwrap();
    let mut settings = settings();
    settings.mode = Mode::Advanced;
    settings.workflows.enabled = true;
    let session = AgentSession::new(
        manager,
        core_tools(directory.path()),
        Registry::from_builtin(),
        settings,
        "main prompt".into(),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(|_| {}),
    );
    (directory, session)
}

#[tokio::test]
async fn every_tool_has_its_interface_and_placeholder_descriptions() {
    let (_directory, session) = main_session();
    let interfaces = interfaces();
    let tools = session.offered_tools();
    let mut offered: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    offered.sort_unstable();
    let mut expected: Vec<&str> = interfaces.keys().map(String::as_str).collect();
    expected.sort_unstable();
    assert_eq!(offered, expected, "the advanced mode offers the 14 tools");
    for tool in &tools {
        let name = tool.name();
        assert_eq!(tool.description(), tool_description(name), "{name}");
        let mut schema = tool.parameters();
        without_descriptions(name, "", &mut schema);
        assert_eq!(schema, interfaces[name], "the schema of {name}");
    }
}

#[tokio::test]
async fn the_basic_mode_offers_the_core_tools_with_the_same_schemas() {
    let (_directory, session) = main_session();
    let advanced: Vec<(String, Value)> = session
        .offered_tools()
        .iter()
        .map(|tool| (tool.name().to_string(), tool.parameters()))
        .collect();
    let mut settings = session.settings();
    settings.mode = Mode::Basic;
    session.update_settings(settings);
    let basic: Vec<(String, Value)> = session
        .offered_tools()
        .iter()
        .map(|tool| (tool.name().to_string(), tool.parameters()))
        .collect();
    let names: Vec<&str> = basic.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["Read", "Write", "Edit", "Bash"]);
    for (name, schema) in &basic {
        let (_, same) = advanced
            .iter()
            .find(|(advanced, _)| advanced == name)
            .unwrap();
        assert_eq!(schema, same, "{name} is one tool in both modes");
    }
}
