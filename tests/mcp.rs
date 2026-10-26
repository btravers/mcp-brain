use anyhow::Result;
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn stdio_client_discovers_tools_reads_captures_and_observes_rejections() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30),async {
        let temp=tempfile::tempdir()?;
        let root=temp.path().join("vault");
        let state=temp.path().join("state");
        std::fs::create_dir_all(root.join("0 Inbox"))?;
        let original="# Human\nMy words stay.\n";
        std::fs::write(root.join("0 Inbox/Human.md"),original)?;
        let mut command=tokio::process::Command::new(env!("CARGO_BIN_EXE_mcp-brain"));
        command.arg("--vault").arg(&root).arg("--state-dir").arg(&state).arg("serve");
        let client=().serve(TokioChildProcess::new(command)?).await?;
        let tools=client.list_all_tools().await?;
        assert_eq!(tools.len(),19);
        assert!(!tools.iter().any(|t|t.name.contains("apply")||t.name.contains("delete")||t.name.contains("configure")));
        assert!(tools.iter().any(|t|t.name=="read_note" && t.annotations.as_ref().unwrap().read_only_hint==Some(true)));
        assert!(tools.iter().any(|t|t.name=="inbox_plan_review" && t.annotations.as_ref().unwrap().read_only_hint==Some(true)));
        let context=client.call_tool(CallToolRequestParams::new("vault_context")).await?;
        assert_eq!(context.structured_content.as_ref().unwrap()["inbox"]["count"],1);
        let read=client.call_tool(CallToolRequestParams::new("read_note").with_arguments(json!({"path":"0 Inbox/Human.md"}).as_object().unwrap().clone())).await?;
        let revision=read.structured_content.as_ref().unwrap()["revision"].as_str().unwrap();
        let forbidden=client.call_tool(CallToolRequestParams::new("update_ai_note").with_arguments(json!({"path":"0 Inbox/Human.md","expected_revision":revision,"content":"Overwrite"}).as_object().unwrap().clone())).await?;
        assert_eq!(forbidden.is_error,Some(true));
        assert_eq!(std::fs::read_to_string(root.join("0 Inbox/Human.md"))?,original);
        let comment=client.call_tool(CallToolRequestParams::new("comment_note").with_arguments(json!({"path":"0 Inbox/Human.md","expected_revision":revision,"content":"A suggestion."}).as_object().unwrap().clone())).await?;
        assert_ne!(comment.is_error,Some(true));
        assert!(std::fs::read_to_string(root.join("0 Inbox/Human.md"))?.starts_with(original));
        let capture=client.call_tool(CallToolRequestParams::new("create_note").with_arguments(json!({"filename":"Memory.md","title":"Memory","body":"An invented memory."}).as_object().unwrap().clone())).await?;
        assert_ne!(capture.is_error,Some(true),"{capture:?}");
        let vault=mcp_brain::vault::Vault::open(&root,Some(state.clone()))?;
        let proposal=vault.propose_plan(vec![mcp_brain::review::Move{
            source:"0 Inbox/Memory.md".into(),destination:"3 Resources/Memory.md".into(),
            expected_revision:mcp_brain::note::revision(&vault.read_text("0 Inbox/Memory.md")?),
            reason:"Reference for future study".into(),
        }])?;
        let id=proposal["id"].as_str().unwrap();
        vault.record_review(id,0,mcp_brain::review::Decision::Commented,Some("This is an ongoing responsibility; suggest an Area.".into()))?;
        let before=vault.snapshot()?;
        let feedback=client.call_tool(CallToolRequestParams::new("inbox_plan_review").with_arguments(json!({"id":id}).as_object().unwrap().clone())).await?;
        assert_eq!(feedback.structured_content.as_ref().unwrap()["items"][0]["decision"],"commented");
        assert!(feedback.structured_content.as_ref().unwrap()["items"][0]["comment"].as_str().unwrap().contains("Area"));
        assert_eq!(vault.snapshot()?,before);
        let forbidden_review=client.call_tool(CallToolRequestParams::new("inbox_plan_review").with_arguments(json!({"id":id,"decision":"accepted"}).as_object().unwrap().clone())).await;
        assert!(forbidden_review.is_err()||forbidden_review.as_ref().is_ok_and(|r|r.is_error==Some(true)));
        let item=vault.review_item_plan(id,0)?;
        vault.apply_review_item(id,0,&item.id)?;
        let applied=client.call_tool(CallToolRequestParams::new("inbox_plan_review").with_arguments(json!({"id":id}).as_object().unwrap().clone())).await?;
        assert_eq!(applied.structured_content.as_ref().unwrap()["applied_count"],1);
        assert_eq!(applied.structured_content.as_ref().unwrap()["items"][0]["decision"],"applied");
        let undo=vault.review_undo_plan(id)?;
        vault.undo_review_item(id,&undo.id)?;
        let undone=client.call_tool(CallToolRequestParams::new("inbox_plan_review").with_arguments(json!({"id":id}).as_object().unwrap().clone())).await?;
        let feedback=undone.structured_content.as_ref().unwrap();
        assert_eq!(feedback["applied_count"],0);
        assert_eq!(feedback["items"][0]["decision"],"commented");
        assert!(feedback["items"][0]["comment"].as_str().unwrap().contains("Area"));
        assert_eq!(feedback["undo_available"],false);
        assert_eq!(vault.snapshot()?,before);
        let search=client.call_tool(CallToolRequestParams::new("search_notes").with_arguments(json!({"query":"invented"}).as_object().unwrap().clone())).await?;
        assert_eq!(search.structured_content.as_ref().unwrap()["total"],1);
        let sync=client.call_tool(CallToolRequestParams::new("sync_status")).await?;
        assert_eq!(sync.structured_content.as_ref().unwrap()["git"]["enabled"],false);
        let review=client.call_tool(CallToolRequestParams::new("memory_review").with_arguments(serde_json::Map::new())).await?;
        assert_eq!(review.structured_content.as_ref().unwrap()["total"],0);
        let sync=client.call_tool(CallToolRequestParams::new("sync_vault")).await?;
        assert_eq!(sync.is_error,Some(true));
        let invalid=client.call_tool(CallToolRequestParams::new("create_note").with_arguments(json!({"filename":"Other.md","title":"Other","body":"Body","author":"human"}).as_object().unwrap().clone())).await;
        assert!(invalid.is_err()||invalid.as_ref().is_ok_and(|r|r.is_error==Some(true)));
        client.cancel().await?;
        Ok::<_,anyhow::Error>(())
    }).await??;
    Ok(())
}

#[tokio::test]
async fn legacy_initialize_and_json_lines_work_without_stdout_noise() -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    tokio::time::timeout(Duration::from_secs(15),async {
        let temp=tempfile::tempdir()?;
        let root=temp.path().join("vault");
        std::fs::create_dir(&root)?;
        let mut child=tokio::process::Command::new(env!("CARGO_BIN_EXE_mcp-brain"))
            .arg("--vault").arg(&root).arg("--state-dir").arg(temp.path().join("state")).arg("serve")
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).kill_on_drop(true).spawn()?;
        let mut stdin=child.stdin.take().unwrap();
        let mut lines=BufReader::new(child.stdout.take().unwrap()).lines();
        let init=json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture-client","version":"1.0"}}});
        stdin.write_all(format!("{init}\n").as_bytes()).await?;
        let response:serde_json::Value=serde_json::from_str(&lines.next_line().await?.unwrap())?;
        assert_eq!(response["result"]["protocolVersion"],"2025-11-25");
        stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await?;
        stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n").await?;
        let response:serde_json::Value=serde_json::from_str(&lines.next_line().await?.unwrap())?;
        assert_eq!(response["result"]["tools"].as_array().unwrap().len(),19);
        child.kill().await?;
        Ok::<_,anyhow::Error>(())
    }).await??;
    Ok(())
}
