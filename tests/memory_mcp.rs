use anyhow::Result;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn memory_survives_a_new_mcp_client_session() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("vault");
        let state = temp.path().join("state");
        std::fs::create_dir(&root)?;
        let transport = || {
            let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_mcp-brain"));
            command.arg("--vault").arg(&root).arg("--state-dir").arg(&state).arg("serve");
            TokioChildProcess::new(command)
        };
        let first = ().serve(transport()?).await?;
        let saved = first.call_tool(CallToolRequestParams::new("remember").with_arguments(
            json!({"title":"Example preference", "content":"Prefer complete examples.", "memory_type":"preference", "source":"User explicitly stated this in a fictional test.", "basis":"user_stated"}).as_object().unwrap().clone()
        )).await?;
        assert_ne!(saved.is_error, Some(true), "{saved:?}");
        let id = saved.structured_content.as_ref().unwrap()["memory"]["id"].clone();
        first.cancel().await?;

        let second = ().serve(transport()?).await?;
        let recalled = second.call_tool(CallToolRequestParams::new("recall").with_arguments(
            json!({"query":"examples"}).as_object().unwrap().clone()
        )).await?;
        assert_ne!(recalled.is_error, Some(true), "{recalled:?}");
        let memory = &recalled.structured_content.as_ref().unwrap()["memories"][0]["memory"];
        assert_eq!(memory["id"], id);
        let archived = second.call_tool(CallToolRequestParams::new("archive_memory").with_arguments(
            json!({"id":id, "expected_revision":memory["revision"], "reason":"User withdrew the preference."}).as_object().unwrap().clone()
        )).await?;
        assert_ne!(archived.is_error, Some(true), "{archived:?}");
        let empty = second.call_tool(CallToolRequestParams::new("recall").with_arguments(serde_json::Map::new())).await?;
        assert_eq!(empty.structured_content.as_ref().unwrap()["total"], 0);
        second.cancel().await?;
        Ok::<_, anyhow::Error>(())
    }).await??;
    Ok(())
}
