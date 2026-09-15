mod common;
use std::fs;

use common::*;

#[tokio::test]
async fn knowledge_base_flow() {
    let app = app().await;
    let user = TestUser::new("kb-flow");

    let unauth_req =
        Request::builder().method("GET").uri("/api/v1/knowledge/knowledge_base/").body(Body::empty()).unwrap();
    let unauth_res = app.clone().oneshot(unauth_req).await.unwrap();
    assert_eq!(unauth_res.status(), StatusCode::UNAUTHORIZED);

    let create_body = serde_json::json!({
        "name": "Integration KB",
        "description": "kb for integration tests",
        "kb_type": "analysis",
        "parent_id": null,
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &user, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let created = response_json(create_res).await;
    let kb_id = created["id"].as_i64().expect("created kb id");

    let list_req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/", &user);
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_bytes = list_res.into_body().collect().await.unwrap().to_bytes();
    let list_json: Value = serde_json::from_slice(&list_bytes).unwrap();
    let list: Vec<Value> = serde_json::from_value(list_json["items"].clone()).unwrap();
    assert!(list.iter().any(|kb| kb["id"].as_i64() == Some(kb_id) && kb["name"].as_str() == Some("Integration KB")));
    assert!(list_json["total"].as_i64().unwrap() >= 1);

    // Create more KBs to exercise pagination
    for i in 0..3 {
        let body = serde_json::json!({
            "name": format!("Integration KB Paged {}", i),
            "description": "kb for pagination tests",
            "kb_type": "analysis",
            "parent_id": null,
            "is_public": false
        });
        let req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &user, body);
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    // Without pagination params, all KBs are returned
    let all_req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/", &user);
    let all_res = app.clone().oneshot(all_req).await.unwrap();
    assert_eq!(all_res.status(), StatusCode::OK);
    let all_json = response_json(all_res).await;
    let total = all_json["total"].as_i64().expect("total");
    assert_eq!(all_json["items"].as_array().unwrap().len() as i64, total);
    assert!(total >= 4);

    // With pagination params, only one page is returned and total reflects the full count.
    // Other tests run concurrently against the shared app, so `total` may grow between requests;
    // only assert bounds instead of exact equality.
    let paged_req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/?page=2&size=2", &user);
    let paged_res = app.clone().oneshot(paged_req).await.unwrap();
    assert_eq!(paged_res.status(), StatusCode::OK);
    let paged_json = response_json(paged_res).await;
    assert!(paged_json["total"].as_i64().unwrap() >= total);
    let paged_items = paged_json["items"].as_array().unwrap();
    assert!(paged_items.len() <= 2);
    let first_page_req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/?page=1&size=2", &user);
    let first_page_res = app.clone().oneshot(first_page_req).await.unwrap();
    let first_page_json = response_json(first_page_res).await;
    let first_page_items = first_page_json["items"].as_array().unwrap();
    assert_eq!(first_page_items.len(), 2);
    let first_ids: Vec<i64> = first_page_items.iter().map(|kb| kb["id"].as_i64().unwrap()).collect();
    assert!(paged_items.iter().all(|kb| !first_ids.contains(&kb["id"].as_i64().unwrap())));

    let update_body = serde_json::json!({
        "parent_id": kb_id
    });
    let update_req =
        authed_json_request("PUT", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &user, update_body);
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn knowledge_base_public_and_reparse() {
    let app = app().await;
    let user = TestUser::new("kb-public");

    let create_body = serde_json::json!({
        "name": "Public KB",
        "description": "kb for public tests",
        "kb_type": "analysis",
        "parent_id": null,
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &user, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let created = response_json(create_res).await;
    let kb_id = created["id"].as_i64().expect("created kb id");

    let public_req = authed_json_request(
        "PUT",
        format!("/api/v1/knowledge/knowledge_base/{}", kb_id),
        &user,
        serde_json::json!({ "is_public": true }),
    );
    let public_res = app.clone().oneshot(public_req).await.unwrap();
    assert_eq!(public_res.status(), StatusCode::OK);
    let public_json = response_json(public_res).await;
    assert_eq!(public_json["is_public"].as_bool(), Some(true));

    let reparse_req = authed_empty_request("POST", "/api/v1/knowledge/knowledge_base/reparse", &user);
    let reparse_res = app.clone().oneshot(reparse_req).await.unwrap();
    assert_eq!(reparse_res.status(), StatusCode::OK);
    let reparse_json = response_json(reparse_res).await;
    assert_eq!(reparse_json["kb_count"].as_i64(), Some(1));
    assert_eq!(reparse_json["file_count"].as_i64(), Some(0));
}

#[tokio::test]
async fn knowledge_base_tree_and_detail_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("kb-tree");

    let root_kb_id = insert_kb(&pool, &user, "Root KB", "analysis", None, false).await;
    let child_kb_id = insert_kb(&pool, &user, "Child KB", "analysis", Some(root_kb_id), false).await;

    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let root_file_path = file_dir.join(format!("root-file-{}.txt", next_seq()));
    let child_file_path = file_dir.join(format!("child-file-{}.txt", next_seq()));
    fs::write(&root_file_path, b"root file").unwrap();
    fs::write(&child_file_path, b"child file").unwrap();

    let root_file_id =
        insert_file(&pool, &user, "root-file.txt", &root_file_path, Some(root_kb_id), vec!["root".to_string()], false)
            .await;
    let child_file_id = insert_file(
        &pool,
        &user,
        "child-file.txt",
        &child_file_path,
        Some(child_kb_id),
        vec!["child".to_string()],
        false,
    )
    .await;

    let tree_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/tree?kb_id={}", root_kb_id), &user);
    let tree_res = app.clone().oneshot(tree_req).await.unwrap();
    assert_eq!(tree_res.status(), StatusCode::OK);
    let tree_json = response_json(tree_res).await;
    let tree_nodes = tree_json.as_array().expect("tree nodes");
    assert_eq!(tree_nodes.len(), 1);
    let root_node = &tree_nodes[0];
    assert_eq!(root_node["id"].as_i64(), Some(root_kb_id));
    assert!(root_node["files"].as_array().unwrap().iter().any(|file| file["id"].as_i64() == Some(root_file_id)));
    assert!(root_node["children"].as_array().unwrap().iter().any(|child| {
        child["id"].as_i64() == Some(child_kb_id)
            && child["files"].as_array().unwrap().iter().any(|file| file["id"].as_i64() == Some(child_file_id))
    }));

    let detail_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}", root_kb_id), &user);
    let detail_res = app.clone().oneshot(detail_req).await.unwrap();
    assert_eq!(detail_res.status(), StatusCode::OK);
    let detail_json = response_json(detail_res).await;
    assert_eq!(detail_json["id"].as_i64(), Some(root_kb_id));
    assert!(detail_json["files"].is_null(), "detail should no longer return files");

    // New paginated files endpoint
    let kb_files_req = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/knowledge_base/{}/files?filename=root-file", root_kb_id),
        &user,
    );
    let kb_files_res = app.clone().oneshot(kb_files_req).await.unwrap();
    assert_eq!(kb_files_res.status(), StatusCode::OK);
    let kb_files_json = response_json(kb_files_res).await;
    let kb_files_items = kb_files_json["items"].as_array().expect("kb files items");
    assert!(kb_files_items.iter().any(|file| file["id"].as_i64() == Some(root_file_id)));
    assert!(kb_files_json["total"].as_i64().unwrap_or(0) >= 1);

    // Pagination
    let kb_files_page_req = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/knowledge_base/{}/files?size=1&page=1", root_kb_id),
        &user,
    );
    let kb_files_page_res = app.clone().oneshot(kb_files_page_req).await.unwrap();
    assert_eq!(kb_files_page_res.status(), StatusCode::OK);
    let kb_files_page_json = response_json(kb_files_page_res).await;
    assert!(kb_files_page_json["total"].as_i64().unwrap_or(0) >= 1);
    assert!(kb_files_page_json["items"].as_array().expect("items").len() <= 1);

    // Tag filter
    let kb_files_tag_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/files?tag=root", root_kb_id), &user);
    let kb_files_tag_res = app.clone().oneshot(kb_files_tag_req).await.unwrap();
    assert_eq!(kb_files_tag_res.status(), StatusCode::OK);
    let kb_files_tag_json = response_json(kb_files_tag_res).await;
    assert!(
        kb_files_tag_json["items"]
            .as_array()
            .expect("tag items")
            .iter()
            .all(|file| { file["id"].as_i64() == Some(root_file_id) })
    );
}

#[tokio::test]
async fn knowledge_base_tag_stats_respect_scope_data_quality_and_permissions() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let owner = TestUser::with_role("tag-owner", "user");
    let viewer = TestUser::with_role("tag-viewer", "user");

    let root_kb_id = insert_kb(&pool, &owner, "Tag Root", "analysis", None, true).await;
    let child_kb_id = insert_kb(&pool, &owner, "Tag Child", "analysis", Some(root_kb_id), false).await;
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();

    let private_path = file_dir.join(format!("tag-private-{}.txt", next_seq()));
    let public_path = file_dir.join(format!("tag-public-{}.txt", next_seq()));
    let child_path = file_dir.join(format!("tag-child-{}.txt", next_seq()));
    let invalid_path = file_dir.join(format!("tag-invalid-{}.txt", next_seq()));
    fs::write(&private_path, b"private").unwrap();
    fs::write(&public_path, b"public").unwrap();
    fs::write(&child_path, b"child").unwrap();
    fs::write(&invalid_path, b"invalid").unwrap();

    insert_file(
        &pool,
        &owner,
        "tag-private.txt",
        &private_path,
        Some(root_kb_id),
        vec!["alpha".to_string(), "shared".to_string(), "shared".to_string()],
        false,
    )
    .await;
    insert_file(
        &pool,
        &owner,
        "tag-public.txt",
        &public_path,
        Some(root_kb_id),
        vec!["alpha".to_string(), "".to_string(), "beta".to_string()],
        true,
    )
    .await;
    insert_file(
        &pool,
        &owner,
        "tag-child.txt",
        &child_path,
        Some(child_kb_id),
        vec!["child".to_string(), "shared".to_string()],
        true,
    )
    .await;
    let invalid_file_id =
        insert_file(&pool, &owner, "tag-invalid.txt", &invalid_path, Some(root_kb_id), vec![], false).await;
    sqlx::query("UPDATE files SET tags = 'not-json' WHERE id = ?").bind(invalid_file_id).execute(&pool).await.unwrap();

    let direct_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/tags", root_kb_id), &owner);
    let direct_res = app.clone().oneshot(direct_req).await.unwrap();
    assert_eq!(direct_res.status(), StatusCode::OK);
    let direct_json = response_json(direct_res).await;
    assert_eq!(direct_json["kb_id"].as_i64(), Some(root_kb_id));
    assert_eq!(
        direct_json["tags"],
        serde_json::json!([
            {"tag": "alpha", "file_count": 2},
            {"tag": "beta", "file_count": 1},
            {"tag": "shared", "file_count": 1}
        ])
    );

    let descendants_req = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/knowledge_base/{}/tags?include_descendants=true", root_kb_id),
        &owner,
    );
    let descendants_res = app.clone().oneshot(descendants_req).await.unwrap();
    assert_eq!(descendants_res.status(), StatusCode::OK);
    let descendants_json = response_json(descendants_res).await;
    assert_eq!(
        descendants_json["tags"],
        serde_json::json!([
            {"tag": "alpha", "file_count": 2},
            {"tag": "shared", "file_count": 2},
            {"tag": "beta", "file_count": 1},
            {"tag": "child", "file_count": 1}
        ])
    );

    let viewer_req = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/knowledge_base/{}/tags?include_descendants=true", root_kb_id),
        &viewer,
    );
    let viewer_res = app.clone().oneshot(viewer_req).await.unwrap();
    assert_eq!(viewer_res.status(), StatusCode::OK);
    let viewer_json = response_json(viewer_res).await;
    assert_eq!(
        viewer_json["tags"],
        serde_json::json!([
            {"tag": "alpha", "file_count": 1},
            {"tag": "beta", "file_count": 1}
        ])
    );

    let inaccessible_kb_id = insert_kb(&pool, &owner, "Tag Private", "analysis", None, false).await;
    let inaccessible_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/tags", inaccessible_kb_id), &viewer);
    let inaccessible_res = app.clone().oneshot(inaccessible_req).await.unwrap();
    assert_eq!(inaccessible_res.status(), StatusCode::NOT_FOUND);

    let admin = TestUser::new("tag-admin");
    let missing_req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/9223372036854775807/tags", &admin);
    let missing_res = app.oneshot(missing_req).await.unwrap();
    assert_eq!(missing_res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn file_endpoints_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("file");

    let kb_id = insert_kb(&pool, &user, "File KB", "analysis", None, false).await;

    let file_suffix = next_seq();
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let file_path = file_dir.join(format!("file-{}.txt", file_suffix));
    let file_contents = b"file contents";
    fs::write(&file_path, file_contents).unwrap();

    let file_id = insert_file(
        &pool,
        &user,
        "file.txt",
        &file_path,
        Some(kb_id),
        vec!["tag1".to_string(), "tag2".to_string()],
        false,
    )
    .await;

    let slice_id = insert_slice(&pool, file_id, "slice content").await;
    insert_slice_position(&pool, slice_id, 1, [1, 2, 3, 4]).await;

    let list_req = authed_empty_request("GET", "/api/v1/knowledge/files/", &user);
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_json = response_json(list_res).await;
    let list = list_json["items"].as_array().expect("file list items");
    assert!(list_json["total"].as_i64().unwrap_or(0) >= 1);
    assert!(list.iter().any(|f| f["id"].as_i64() == Some(file_id)));

    let list_tag_req = authed_empty_request("GET", "/api/v1/knowledge/files/?tag=tag1", &user);
    let list_tag_res = app.clone().oneshot(list_tag_req).await.unwrap();
    assert_eq!(list_tag_res.status(), StatusCode::OK);
    let list_tag_json = response_json(list_tag_res).await;
    let list_tag = list_tag_json["items"].as_array().expect("file list items");
    assert!(list_tag.iter().any(|f| f["id"].as_i64() == Some(file_id)));

    let list_kb_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/?kb_id={}", kb_id), &user);
    let list_kb_res = app.clone().oneshot(list_kb_req).await.unwrap();
    assert_eq!(list_kb_res.status(), StatusCode::OK);
    let list_kb_json = response_json(list_kb_res).await;
    let list_kb = list_kb_json["items"].as_array().expect("file list items");
    assert!(list_kb.iter().any(|f| f["id"].as_i64() == Some(file_id)));

    let get_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/{}", file_id), &user);
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_json = response_json(get_res).await;
    assert_eq!(get_json["id"].as_i64(), Some(file_id));

    let update_body = serde_json::json!({
        "filename": "renamed.txt",
        "tags": ["tag3"]
    });
    let update_req = authed_json_request("PUT", format!("/api/v1/knowledge/files/{}", file_id), &user, update_body);
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::OK);
    let update_json = response_json(update_res).await;
    assert_eq!(update_json["filename"].as_str(), Some("renamed.txt"));
    let updated_tags: Vec<String> = serde_json::from_str(update_json["tags"].as_str().unwrap()).unwrap();
    assert_eq!(updated_tags, vec!["tag3".to_string()]);

    let download_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/{}/download", file_id), &user);
    let download_res = app.clone().oneshot(download_req).await.unwrap();
    assert_eq!(download_res.status(), StatusCode::OK);
    let disposition =
        download_res.headers().get(header::CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(disposition.contains("renamed.txt"));
    let download_bytes = download_res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(download_bytes.as_ref(), file_contents);

    let slices_req = authed_empty_request("GET", format!("/api/v1/knowledge/files/{}/slices", file_id), &user);
    let slices_res = app.clone().oneshot(slices_req).await.unwrap();
    assert_eq!(slices_res.status(), StatusCode::OK);
    let slices_json = response_json(slices_res).await;
    let slices = slices_json.as_array().expect("slice list");
    let slice = slices.iter().find(|s| s["id"].as_i64() == Some(slice_id)).expect("slice entry");
    assert_eq!(slice["file_id"].as_i64(), Some(file_id));
    assert_eq!(slice["content"].as_str(), Some("slice content"));
    assert!(slice["positions"].is_null(), "slices endpoint should not return positions");

    let delete_req = authed_empty_request("DELETE", format!("/api/v1/knowledge/files/{}", file_id), &user);
    let delete_res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(delete_res.status(), StatusCode::OK);
    assert!(!file_path.exists());
}

#[tokio::test]
async fn slice_highlight_by_id() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("slice_highlight");

    let kb_id = insert_kb(&pool, &user, "Slice Highlight KB", "analysis", None, false).await;

    let file_suffix = next_seq();
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let file_path = file_dir.join(format!("highlight-{}.txt", file_suffix));
    fs::write(&file_path, b"highlight content").unwrap();

    let file_id = insert_file(&pool, &user, "highlight.txt", &file_path, Some(kb_id), Vec::new(), false).await;

    let slice_id = insert_slice(&pool, file_id, "slice content").await;
    insert_slice_position(&pool, slice_id, 2, [10, 20, 100, 30]).await;

    // 查询切片高亮信息
    let highlight_req = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/files/{}/slices/{}/highlight", file_id, slice_id),
        &user,
    );
    let highlight_res = app.clone().oneshot(highlight_req).await.unwrap();
    assert_eq!(highlight_res.status(), StatusCode::OK);
    let highlight_json = response_json(highlight_res).await;
    let positions = highlight_json["positions"].as_array().expect("positions array");
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0]["page_idx"].as_i64(), Some(2));
    let bbox = positions[0]["bbox"].as_array().expect("bbox array");
    assert_eq!(bbox.len(), 4);
    assert_eq!(bbox[0].as_i64(), Some(10));
    assert_eq!(bbox[3].as_i64(), Some(30));

    // 切片不属于该文件应返回 400
    let other_file_id = insert_file(&pool, &user, "other.txt", &file_path, Some(kb_id), Vec::new(), false).await;
    let bad_req = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/files/{}/slices/{}/highlight", other_file_id, slice_id),
        &user,
    );
    let bad_res = app.clone().oneshot(bad_req).await.unwrap();
    assert_eq!(bad_res.status(), StatusCode::BAD_REQUEST);

    // 不存在的切片应返回 404
    let missing_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/files/{}/slices/{}/highlight", file_id, 999999), &user);
    let missing_res = app.clone().oneshot(missing_req).await.unwrap();
    assert_eq!(missing_res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn slice_highlight_page_by_id() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("slice_highlight_page");

    let kb_id = insert_kb(&pool, &user, "Slice Highlight Page KB", "analysis", None, false).await;

    let file_suffix = next_seq();
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let file_path = file_dir.join(format!("highlight-page-{}.txt", file_suffix));
    fs::write(&file_path, b"highlight content").unwrap();

    let file_id = insert_file(&pool, &user, "highlight-page.txt", &file_path, Some(kb_id), Vec::new(), false).await;

    // 为文件写入 pdf_content，使 highlight-page 能按内容字数判断
    let pdf_rows = vec![
        htknow::pdf_content::PdfContent {
            page_idx: 0,
            bbox: Some("[10,20,100,30]".to_string()),
            text: Some("a".to_string()),
            text_level: None,
            img_path: None,
            table_body: None,
        },
        htknow::pdf_content::PdfContent {
            page_idx: 1,
            bbox: Some("[10,20,100,30]".to_string()),
            text: Some("a".to_string()),
            text_level: None,
            img_path: None,
            table_body: None,
        },
    ];
    htknow::pdf_content::write(file_id, &pdf_rows).await.unwrap();

    // 场景 1：第一页内容字数少于阈值（19 * 1 = 19 < 20）且存在第二页，应返回第二页
    let slice_id_a = insert_slice(&pool, file_id, "slice a").await;
    for _ in 0..19 {
        insert_slice_position(&pool, slice_id_a, 0, [10, 20, 100, 30]).await;
    }
    insert_slice_position(&pool, slice_id_a, 1, [10, 20, 100, 30]).await;

    let req_a = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/files/{}/slices/{}/highlight-page", file_id, slice_id_a),
        &user,
    );
    let res_a = app.clone().oneshot(req_a).await.unwrap();
    assert_eq!(res_a.status(), StatusCode::OK);
    let json_a = response_json(res_a).await;
    assert_eq!(json_a["page_idx"].as_i64(), Some(1));

    // 场景 2：第一页内容字数达到阈值（21 * 1 = 21 >= 20），应返回第一页
    let slice_id_b = insert_slice(&pool, file_id, "slice b").await;
    for _ in 0..21 {
        insert_slice_position(&pool, slice_id_b, 0, [10, 20, 100, 30]).await;
    }
    insert_slice_position(&pool, slice_id_b, 1, [10, 20, 100, 30]).await;

    let req_b = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/files/{}/slices/{}/highlight-page", file_id, slice_id_b),
        &user,
    );
    let res_b = app.clone().oneshot(req_b).await.unwrap();
    assert_eq!(res_b.status(), StatusCode::OK);
    let json_b = response_json(res_b).await;
    assert_eq!(json_b["page_idx"].as_i64(), Some(0));

    // 场景 3：只有一页且内容字数少于阈值（无 pdf_content 匹配，按位置数量兜底为 1），仍返回第一页
    let slice_id_c = insert_slice(&pool, file_id, "slice c").await;
    insert_slice_position(&pool, slice_id_c, 2, [10, 20, 100, 30]).await;

    let req_c = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/files/{}/slices/{}/highlight-page", file_id, slice_id_c),
        &user,
    );
    let res_c = app.clone().oneshot(req_c).await.unwrap();
    assert_eq!(res_c.status(), StatusCode::OK);
    let json_c = response_json(res_c).await;
    assert_eq!(json_c["page_idx"].as_i64(), Some(2));

    // 场景 4：切片没有高亮位置应返回 404
    let slice_id_d = insert_slice(&pool, file_id, "slice d").await;
    let req_d = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/files/{}/slices/{}/highlight-page", file_id, slice_id_d),
        &user,
    );
    let res_d = app.clone().oneshot(req_d).await.unwrap();
    assert_eq!(res_d.status(), StatusCode::NOT_FOUND);

    // 场景 5：切片不属于该文件应返回 400
    let other_file_id = insert_file(&pool, &user, "other.txt", &file_path, Some(kb_id), Vec::new(), false).await;
    let req_bad = authed_empty_request(
        "GET",
        format!("/api/v1/knowledge/files/{}/slices/{}/highlight-page", other_file_id, slice_id_a),
        &user,
    );
    let res_bad = app.clone().oneshot(req_bad).await.unwrap();
    assert_eq!(res_bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn graph_endpoints_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let env = setup_env();
    let user = TestUser::new("graph");

    let kb_id = insert_kb(&pool, &user, "Graph KB", "analysis", None, false).await;

    let file_suffix = next_seq();
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let file_path = file_dir.join(format!("graph-{}.txt", file_suffix));
    fs::write(&file_path, b"graph file").unwrap();

    let file_id = insert_file(&pool, &user, "graph.txt", &file_path, Some(kb_id), Vec::new(), false).await;
    let slice_id = insert_slice(&pool, file_id, "mention context").await;

    let node_a_id = insert_graph_node(
        &pool,
        "Alpha",
        "device",
        Some(serde_json::json!({ "origin": "test" })),
        Some(file_id),
        Some(kb_id),
    )
    .await;
    let node_b_id = insert_graph_node(&pool, "Beta", "component", None, Some(file_id), Some(kb_id)).await;
    insert_graph_edge(&pool, node_a_id, node_b_id, "related_to", Some(file_id)).await;
    insert_entity_mention(&pool, node_a_id, slice_id, "Alpha mention").await;

    let search_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/graph/entities?kb_id={}&q=Alpha", kb_id), &user);
    let search_res = app.clone().oneshot(search_req).await.unwrap();
    assert_eq!(search_res.status(), StatusCode::OK);
    let search_json = response_json(search_res).await;
    let search_list = search_json.as_array().expect("entity list");
    assert!(search_list.iter().any(|entity| entity["id"].as_i64() == Some(node_a_id)));

    let entity_req = authed_empty_request("GET", format!("/api/v1/knowledge/graph/entities/{}", node_a_id), &user);
    let entity_res = app.clone().oneshot(entity_req).await.unwrap();
    assert_eq!(entity_res.status(), StatusCode::OK);
    let entity_json = response_json(entity_res).await;
    assert_eq!(entity_json["entity"]["id"].as_i64(), Some(node_a_id));
    let neighbors = entity_json["neighbors"].as_array().expect("neighbors");
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0]["entity"]["id"].as_i64(), Some(node_b_id));
    assert_eq!(neighbors[0]["direction"].as_str(), Some("outgoing"));
    let mentions = entity_json["mentions"].as_array().expect("mentions");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0]["file_id"].as_i64(), Some(file_id));

    let stats_req = authed_empty_request("GET", format!("/api/v1/knowledge/graph/stats?kb_id={}", kb_id), &user);
    let stats_res = app.clone().oneshot(stats_req).await.unwrap();
    assert_eq!(stats_res.status(), StatusCode::OK);
    let stats_json = response_json(stats_res).await;
    assert_eq!(stats_json["node_count"].as_i64(), Some(2));
    assert_eq!(stats_json["edge_count"].as_i64(), Some(1));
    let entity_types = stats_json["entity_types"].as_object().expect("entity types");
    assert_eq!(entity_types.get("device").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(entity_types.get("component").and_then(|v| v.as_i64()), Some(1));
    let relation_types = stats_json["relation_types"].as_object().expect("relation types");
    assert_eq!(relation_types.get("related_to").and_then(|v| v.as_i64()), Some(1));
}

#[tokio::test]
async fn removed_search_modes_and_image_requires_file() {
    let app = app().await;
    let user = TestUser::new("search");

    let full_req = authed_empty_request("GET", "/api/v1/knowledge/search/full?query=missing", &user);
    let full_res = app.clone().oneshot(full_req).await.unwrap();
    assert_eq!(full_res.status(), StatusCode::NOT_FOUND);
    let spec = serde_json::to_value(htknow::api::openapi()).unwrap();
    assert!(spec["paths"].get("/api/v1/knowledge/search/full").is_none());
    assert!(spec["paths"].get("/api/v1/knowledge/search/summary").is_some());
    let advanced_req = authed_empty_request("GET", "/api/v1/knowledge/search/advanced/stream?query=missing", &user);
    let advanced_res = app.clone().oneshot(advanced_req).await.unwrap();
    assert_eq!(advanced_res.status(), StatusCode::NOT_FOUND);
    assert!(spec["paths"].get("/api/v1/knowledge/search/advanced/stream").is_none());
    let parameters = spec["paths"]["/api/v1/knowledge/search/"]["get"]["parameters"].as_array().unwrap();
    assert!(parameters.iter().all(|parameter| parameter["name"] != "advanced"));

    let boundary = format!("boundary-{}", next_seq());
    let body = multipart_body(&boundary, &[("text", "sample")]);
    let image_req = authed_multipart_request("POST", "/api/v1/knowledge/search/image", &user, &boundary, body);
    let image_res = app.clone().oneshot(image_req).await.unwrap();
    assert_eq!(image_res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn kb_permission_viewer_can_read_but_not_modify() {
    let app = app().await;
    let owner = TestUser::with_role("kb-perm-owner", "user");
    let viewer = TestUser::with_role("kb-perm-viewer", "user");

    // Owner creates a private KB
    let create_body = serde_json::json!({
        "name": "Permission Test KB",
        "description": "kb for permission tests",
        "kb_type": "analysis",
        "parent_id": null,
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &owner, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let created = response_json(create_res).await;
    let kb_id = created["id"].as_i64().expect("created kb id");
    assert_eq!(created["current_user_permission"].as_str(), Some("admin"));

    // Owner grants viewer permission
    let grant_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": viewer.id, "permission": "viewer" }),
    );
    let grant_res = app.clone().oneshot(grant_req).await.unwrap();
    assert_eq!(grant_res.status(), StatusCode::OK);

    // Viewer can list the KB
    let list_req = authed_empty_request("GET", "/api/v1/knowledge/knowledge_base/", &viewer);
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_bytes = list_res.into_body().collect().await.unwrap().to_bytes();
    let list_json: Value = serde_json::from_slice(&list_bytes).unwrap();
    let list: Vec<Value> = serde_json::from_value(list_json["items"].clone()).unwrap();
    let kb = list.iter().find(|k| k["id"].as_i64() == Some(kb_id)).expect("viewer sees the kb");
    assert_eq!(kb["current_user_permission"].as_str(), Some("viewer"));

    // Viewer can get detail
    let get_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &viewer);
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_json = response_json(get_res).await;
    assert_eq!(get_json["current_user_permission"].as_str(), Some("viewer"));

    // Viewer cannot update the KB
    let update_req = authed_json_request(
        "PUT",
        format!("/api/v1/knowledge/knowledge_base/{}", kb_id),
        &viewer,
        serde_json::json!({ "name": "Hacked" }),
    );
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::FORBIDDEN);

    // Viewer cannot reparse
    let reparse_req =
        authed_empty_request("POST", format!("/api/v1/knowledge/knowledge_base/{}/reparse", kb_id), &viewer);
    let reparse_res = app.clone().oneshot(reparse_req).await.unwrap();
    assert_eq!(reparse_res.status(), StatusCode::FORBIDDEN);

    // Viewer cannot delete
    let delete_req = authed_empty_request("DELETE", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &viewer);
    let delete_res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(delete_res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn kb_permission_editor_can_upload_but_not_delete() {
    let app = app().await;
    let env = setup_env();
    let owner = TestUser::with_role("kb-editor-owner", "user");
    let editor = TestUser::with_role("kb-editor-editor", "user");

    // Owner creates a private storage KB (so we can upload without parse complications)
    let create_body = serde_json::json!({
        "name": "Editor Test KB",
        "description": "kb for editor permission tests",
        "kb_type": "storage",
        "parent_id": null,
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &owner, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let kb_id = response_json(create_res).await["id"].as_i64().unwrap();

    // Owner grants editor permission
    let grant_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": editor.id, "permission": "editor" }),
    );
    let grant_res = app.clone().oneshot(grant_req).await.unwrap();
    assert_eq!(grant_res.status(), StatusCode::OK);

    // Editor can update the KB name/description
    let update_req = authed_json_request(
        "PUT",
        format!("/api/v1/knowledge/knowledge_base/{}", kb_id),
        &editor,
        serde_json::json!({ "name": "Renamed by Editor", "description": "updated" }),
    );
    let update_res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(update_res.status(), StatusCode::OK);

    // Editor cannot change visibility
    let vis_req = authed_json_request(
        "PUT",
        format!("/api/v1/knowledge/knowledge_base/{}", kb_id),
        &editor,
        serde_json::json!({ "is_public": true }),
    );
    let vis_res = app.clone().oneshot(vis_req).await.unwrap();
    assert_eq!(vis_res.status(), StatusCode::FORBIDDEN);

    // Editor can upload a file
    let file_dir = env.data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let test_file = file_dir.join(format!("editor-upload-{}.txt", next_seq()));
    fs::write(&test_file, b"editor upload test").unwrap();

    let boundary = format!("boundary-{}", next_seq());
    let upload_req = authed_multipart_request_with_file(
        "POST",
        "/api/v1/knowledge/files/",
        &editor,
        &boundary,
        &[("kb_id", &kb_id.to_string()), ("slice_type", "text")],
        "file",
        "test.txt",
        b"editor upload test",
    );
    let upload_res = app.clone().oneshot(upload_req).await.unwrap();
    assert_eq!(upload_res.status(), StatusCode::OK);

    // Editor cannot delete the KB
    let delete_req = authed_empty_request("DELETE", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &editor);
    let delete_res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(delete_res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn kb_permission_admin_can_manage_permissions() {
    let app = app().await;
    let owner = TestUser::with_role("kb-admin-owner", "user");
    let viewer = TestUser::with_role("kb-admin-viewer", "user");

    // Owner creates KB
    let create_body = serde_json::json!({
        "name": "Admin Perm Test KB",
        "description": "test",
        "kb_type": "analysis",
        "is_public": false
    });
    let create_req = authed_json_request("POST", "/api/v1/knowledge/knowledge_base/", &owner, create_body);
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    let kb_id = response_json(create_res).await["id"].as_i64().unwrap();

    // Owner lists permissions (should be empty initially except no explicit rows)
    let list_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id), &owner);
    let list_res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);

    // Owner grants viewer permission
    let grant_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": viewer.id, "permission": "viewer" }),
    );
    let grant_res = app.clone().oneshot(grant_req).await.unwrap();
    assert_eq!(grant_res.status(), StatusCode::OK);
    let grant_json = response_json(grant_res).await;
    assert_eq!(grant_json["user_id"].as_str(), Some(viewer.id.as_str()));
    assert_eq!(grant_json["permission"].as_str(), Some("viewer"));

    // Viewer cannot call permission APIs
    let viewer_list_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id), &viewer);
    let viewer_list_res = app.clone().oneshot(viewer_list_req).await.unwrap();
    assert_eq!(viewer_list_res.status(), StatusCode::FORBIDDEN);

    // Owner upgrades viewer to admin
    let upgrade_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": viewer.id, "permission": "admin" }),
    );
    let upgrade_res = app.clone().oneshot(upgrade_req).await.unwrap();
    assert_eq!(upgrade_res.status(), StatusCode::OK);

    // Now viewer (as KB admin) can list permissions
    let list2_req =
        authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id), &viewer);
    let list2_res = app.clone().oneshot(list2_req).await.unwrap();
    assert_eq!(list2_res.status(), StatusCode::OK);

    // Owner removes viewer permission
    let remove_req = authed_empty_request(
        "DELETE",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions/{}", kb_id, viewer.id),
        &owner,
    );
    let remove_res = app.clone().oneshot(remove_req).await.unwrap();
    assert_eq!(remove_res.status(), StatusCode::OK);

    // After removal, viewer can no longer access the KB
    let get_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &viewer);
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn kb_permission_search_filters_unauthorized_kb() {
    let app = app().await;
    let pool = get_pool().await;
    let owner = TestUser::with_role("kb-search-owner", "user");
    let other = TestUser::with_role("kb-search-other", "user");

    // Owner creates a private KB with a file
    let kb_id = insert_kb(&pool, &owner, "Search Permission KB", "analysis", None, false).await;
    let file_dir = setup_env().data_dir.join("files");
    fs::create_dir_all(&file_dir).unwrap();
    let test_file = file_dir.join(format!("search-perm-{}.txt", next_seq()));
    fs::write(&test_file, b"secret content about dragons").unwrap();
    let file_id = insert_file(&pool, &owner, "secret-perm.txt", &test_file, Some(kb_id), vec![], false).await;

    sqlx::query("UPDATE files SET status = 1, summary = 'secret content about dragons' WHERE id = ?")
        .bind(file_id)
        .execute(&pool)
        .await
        .unwrap();

    // Owner can use the summary-search filename filter and find the file
    let owner_search_req =
        authed_empty_request("GET", "/api/v1/knowledge/search/summary?filename=secret-perm.txt", &owner);
    let owner_search_res = app.clone().oneshot(owner_search_req).await.unwrap();
    assert_eq!(owner_search_res.status(), StatusCode::OK);
    let owner_json = response_json(owner_search_res).await;
    let owner_results = owner_json["results"].as_array().expect("results");
    assert!(
        owner_results.iter().any(|r| r["file"]["id"].as_i64() == Some(file_id)),
        "owner should find their own file"
    );

    // Other user without permission cannot see the result
    let other_search_req =
        authed_empty_request("GET", "/api/v1/knowledge/search/summary?filename=secret-perm.txt", &other);
    let other_search_res = app.clone().oneshot(other_search_req).await.unwrap();
    assert_eq!(other_search_res.status(), StatusCode::OK);
    let other_json = response_json(other_search_res).await;
    let other_results = other_json["results"].as_array().expect("results");
    assert!(
        !other_results.iter().any(|r| r["file"]["id"].as_i64() == Some(file_id)),
        "other user should not see unauthorized file"
    );

    // Grant viewer permission and verify search now returns result
    let grant_req = authed_json_request(
        "POST",
        format!("/api/v1/knowledge/knowledge_base/{}/permissions", kb_id),
        &owner,
        serde_json::json!({ "user_id": other.id, "permission": "viewer" }),
    );
    let grant_res = app.clone().oneshot(grant_req).await.unwrap();
    assert_eq!(grant_res.status(), StatusCode::OK);

    // Verify other can now access the KB detail
    let get_req = authed_empty_request("GET", format!("/api/v1/knowledge/knowledge_base/{}", kb_id), &other);
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_json = response_json(get_res).await;
    assert_eq!(get_json["current_user_permission"].as_str(), Some("viewer"));

    let granted_search_req =
        authed_empty_request("GET", "/api/v1/knowledge/search/summary?filename=secret-perm.txt", &other);
    let granted_search_res = app.clone().oneshot(granted_search_req).await.unwrap();
    assert_eq!(granted_search_res.status(), StatusCode::OK);
    let granted_json = response_json(granted_search_res).await;
    let granted_results = granted_json["results"].as_array().expect("results");
    assert!(
        granted_results.iter().any(|r| r["file"]["id"].as_i64() == Some(file_id)),
        "viewer should now find the file"
    );
}

#[tokio::test]
async fn wiki_endpoints_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let owner = TestUser::new("wiki-flow");
    let kb_id = insert_kb(&pool, &owner, "wiki-kb", "analysis", None, false).await;

    // 全局默认关闭时，未显式配置的知识库不生成 Wiki。
    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/config?kb_id={}", kb_id), &owner))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let config = response_json(res).await;
    assert_eq!(config["enabled"].as_bool(), Some(false));
    assert_eq!(config["llm_available"].as_bool(), Some(true));

    // 关闭状态下重建会被拒绝，不会留下没人处理的任务。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/rebuild",
            &owner,
            serde_json::json!({ "kb_id": kb_id }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 非法粒度直接 400，避免写入无法解析的配置。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/config",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "granularity": "nope" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 打开知识库级开关，并回读合并后的生效值。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/config",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "enabled": true, "granularity": "focused", "max_pages_per_ingest": 3 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let config = response_json(res).await;
    assert_eq!(config["enabled"].as_bool(), Some(true));
    assert_eq!(config["granularity"].as_str(), Some("focused"));
    assert_eq!(config["max_pages_per_ingest"].as_u64(), Some(3));

    // 私有库的非成员：读接口 404（不泄露存在性），写接口 403。
    let stranger = TestUser::with_role("wiki-stranger", "user");
    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/index?kb_id={}", kb_id), &stranger))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/config",
            &stranger,
            serde_json::json!({ "kb_id": kb_id, "enabled": false }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // 直接落一个页面，验证只读接口与「切片 → 用户可见文件」的换算。
    let path = std::path::PathBuf::from("data/wiki-doc.md");
    let file_id = insert_file(&pool, &owner, "wiki-doc.md", &path, Some(kb_id), vec![], false).await;
    sqlx::query("UPDATE files SET status = 1 WHERE id = ?").bind(file_id).execute(&pool).await.unwrap();
    let slice_id = insert_slice(&pool, file_id, "张三负责检索系统的索引与召回调优。").await;
    htknow::wiki::page::upsert(
        &pool,
        &htknow::wiki::page::PageDraft {
            kb_id,
            slug: "entity/zhang-san".to_string(),
            title: "张三".to_string(),
            page_type: htknow::wiki::PAGE_TYPE_ENTITY.to_string(),
            summary: "检索系统工程师".to_string(),
            content: "张三 负责检索系统。".to_string(),
            aliases: vec!["Zhang San".to_string()],
            edit_source: htknow::wiki::EDIT_SOURCE_PIPELINE.to_string(),
            editor_id: String::new(),
        },
        &[file_id],
        &[slice_id],
    )
    .await
    .unwrap();

    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/pages?kb_id={}", kb_id), &owner))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pages = response_json(res).await;
    assert_eq!(pages["items"][0]["slug"].as_str(), Some("entity/zhang-san"));
    assert_eq!(pages["items"][0]["aliases"][0].as_str(), Some("Zhang San"));

    let res = app
        .clone()
        .oneshot(authed_empty_request(
            "GET",
            format!("/api/v1/knowledge/wiki/page?kb_id={}&slug=entity/zhang-san", kb_id),
            &owner,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let detail = response_json(res).await;
    assert_eq!(detail["content"].as_str(), Some("张三 负责检索系统。"));
    assert_eq!(detail["sources"][0]["file_id"].as_i64(), Some(file_id));
    assert_eq!(detail["sources"][0]["filename"].as_str(), Some("wiki-doc.md"));
    assert_eq!(detail["slices"][0]["slice_id"].as_i64(), Some(slice_id));
    assert_eq!(detail["slices"][0]["file_id"].as_i64(), Some(file_id));

    let res = app
        .clone()
        .oneshot(authed_empty_request(
            "GET",
            format!("/api/v1/knowledge/wiki/page?kb_id={}&slug=entity/missing", kb_id),
            &owner,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = app
        .clone()
        .oneshot(authed_empty_request(
            "GET",
            format!("/api/v1/knowledge/wiki/search?kb_id={}&q={}", kb_id, "%E5%BC%A0%E4%B8%89"),
            &owner,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let search = response_json(res).await;
    assert_eq!(search["items"].as_array().unwrap().len(), 1);

    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/index?kb_id={}", kb_id), &owner))
        .await
        .unwrap();
    let index = response_json(res).await;
    assert_eq!(index["groups"][0]["items"][0]["slug"].as_str(), Some("entity/zhang-san"));

    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/stats?kb_id={}", kb_id), &owner))
        .await
        .unwrap();
    let stats = response_json(res).await;
    assert_eq!(stats["total"].as_i64(), Some(1));
    assert_eq!(stats["source_file_count"].as_i64(), Some(1));

    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/graph?kb_id={}", kb_id), &owner))
        .await
        .unwrap();
    let graph = response_json(res).await;
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 1);

    // 重建：为已完成解析的文件入队一条 ingest 任务（集成测试不启动 worker，任务留在队列里）。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/rebuild",
            &owner,
            serde_json::json!({ "kb_id": kb_id }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(response_json(res).await["enqueued"].as_i64(), Some(1));
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wiki_tasks WHERE kb_id = ? AND task_type = 'wiki:ingest' AND status = 'pending'",
    )
    .bind(kb_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 1);
}

/// P2：人工编辑 → 版本快照 → 回滚 → 体检 → 链接收敛 → 归档 → 删除。
#[tokio::test]
async fn wiki_edit_and_revision_flow() {
    let app = app().await;
    let pool = get_pool().await;
    let owner = TestUser::new("wiki-edit");
    let kb_id = insert_kb(&pool, &owner, "wiki-edit-kb", "analysis", None, false).await;
    // 公开库：非成员是 viewer，可读版本但不能改。
    let public_kb = insert_kb(&pool, &owner, "wiki-public-kb", "analysis", None, true).await;

    // 手工建页：slug 由标题派生，作者标记为 user。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({
                "kb_id": kb_id,
                "title": "检索增强生成",
                // 显式 slug 会被规范化并补上类型前缀；用 ASCII 省去 URL 编码。
                "slug": "RAG Notes",
                "summary": "一句话摘要",
                "content": "初版正文",
                "aliases": ["RAG", "RAG"],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let created = response_json(res).await;
    assert_eq!(created["page"]["slug"].as_str(), Some("concept/rag-notes"));
    assert_eq!(created["page"]["page_type"].as_str(), Some("concept"));
    assert_eq!(created["page"]["status"].as_str(), Some("published"));
    assert_eq!(created["page"]["last_edit_source"].as_str(), Some("user"));
    assert_eq!(created["page"]["version"].as_i64(), Some(1));
    assert_eq!(created["page"]["aliases"].as_array().unwrap().len(), 1, "别名应去重");
    let slug = "concept/rag-notes";
    let uri = |tail: &str| format!("/api/v1/knowledge/wiki/{}kb_id={}&slug={}", tail, kb_id, slug);

    // 同 slug 再建 → 400（冲突）。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "title": "另一个标题", "slug": "rag-notes" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 摘要页由系统维护，不允许手工创建。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "title": "文档", "page_type": "summary" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 局部编辑：只改正文与摘要，标题保持原样。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({
                "kb_id": kb_id,
                "slug": slug,
                "summary": "改过的摘要",
                "content": "第二版正文 [[concept/不存在|断链]]",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let edited = response_json(res).await;
    assert_eq!(edited["content"].as_str(), Some("第二版正文 [[concept/不存在|断链]]"));
    assert_eq!(edited["page"]["title"].as_str(), Some("检索增强生成"));
    assert_eq!(edited["page"]["summary"].as_str(), Some("改过的摘要"));
    assert_eq!(edited["page"]["version"].as_i64(), Some(2));
    assert_eq!(edited["page"]["out_links"][0].as_str(), Some("concept/不存在"));

    // 非法状态 → 400，不会留下半截写入。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "slug": slug, "status": "bogus" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    // 空编辑 → 400。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "slug": slug }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 版本列表：新→旧，且带当前版本号供前端标注「最新」。
    let res = app.clone().oneshot(authed_empty_request("GET", uri("revisions?"), &owner)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let revisions = response_json(res).await;
    assert_eq!(revisions["current_version"].as_i64(), Some(2));
    let items = revisions["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["version"].as_i64(), Some(1));
    assert_eq!(items[0]["edit_source"].as_str(), Some("user"));
    assert_eq!(items[0]["content_length"].as_i64(), Some(4));
    assert!(items[0].get("content").is_none(), "列表不该带正文");

    // 版本详情：同时给出当前正文，前端一次请求就能 diff。
    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("{}&version=1", uri("revision?")), &owner))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let snapshot = response_json(res).await;
    assert_eq!(snapshot["revision"]["content"].as_str(), Some("初版正文"));
    assert_eq!(snapshot["revision"]["summary"].as_str(), Some("一句话摘要"));
    assert_eq!(snapshot["current_content"].as_str(), Some("第二版正文 [[concept/不存在|断链]]"));
    assert_eq!(snapshot["current_version"].as_i64(), Some(2));

    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("{}&version=99", uri("revision?")), &owner))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 体检：报出断链。
    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/lint?kb_id={}", kb_id), &owner))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let lint = response_json(res).await;
    assert_eq!(lint["scanned"].as_u64(), Some(1));
    let issues = lint["issues"].as_array().unwrap();
    assert!(issues.iter().any(
        |issue| issue["kind"].as_str() == Some("broken_link") && issue["target"].as_str() == Some("concept/不存在")
    ));
    assert!(lint["by_kind"].as_array().unwrap().iter().any(|kind| kind["kind"].as_str() == Some("broken_link")));

    // 立即收敛：死链被清掉，出链同步清空。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/rebuild-links",
            &owner,
            serde_json::json!({ "kb_id": kb_id }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let report = response_json(res).await;
    assert_eq!(report["pages_scanned"].as_u64(), Some(1));
    assert_eq!(report["pages_changed"].as_u64(), Some(1));
    assert_eq!(report["dead_links_removed"].as_u64(), Some(1));

    let res = app.clone().oneshot(authed_empty_request("GET", uri("page?"), &owner)).await.unwrap();
    let healed = response_json(res).await;
    assert_eq!(healed["content"].as_str(), Some("第二版正文 断链"));
    assert!(healed["page"]["out_links"].as_array().unwrap().is_empty());
    // 链接维护不改作者归属，人工页面不会被后续自动生成覆盖。
    assert_eq!(healed["page"]["last_edit_source"].as_str(), Some("user"));
    assert_eq!(healed["page"]["version"].as_i64(), Some(3));

    // 回滚到第一版：内容回来，版本继续往前走。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/revert",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "slug": slug, "version": 1 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let reverted = response_json(res).await;
    assert_eq!(reverted["content"].as_str(), Some("初版正文"));
    assert_eq!(reverted["page"]["version"].as_i64(), Some(4));
    assert_eq!(reverted["page"]["last_edit_source"].as_str(), Some("revert"));
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/revert",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "slug": slug, "version": 99 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 归档：退出目录与搜索，但按状态仍能列出，且随时可恢复。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "slug": slug, "status": "archived" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(response_json(res).await["page"]["status"].as_str(), Some("archived"));

    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/pages?kb_id={}", kb_id), &owner))
        .await
        .unwrap();
    assert!(response_json(res).await["items"].as_array().unwrap().is_empty());
    let res = app
        .clone()
        .oneshot(authed_empty_request(
            "GET",
            format!("/api/v1/knowledge/wiki/pages?kb_id={}&status=archived", kb_id),
            &owner,
        ))
        .await
        .unwrap();
    assert_eq!(response_json(res).await["items"].as_array().unwrap().len(), 1);
    let res = app
        .clone()
        .oneshot(authed_empty_request("GET", format!("/api/v1/knowledge/wiki/stats?kb_id={}", kb_id), &owner))
        .await
        .unwrap();
    let stats = response_json(res).await;
    assert_eq!(stats["archived"].as_i64(), Some(1));
    assert_eq!(stats["published"].as_i64(), Some(0));
    // 归档页仍可读、可回滚。
    let res = app.clone().oneshot(authed_empty_request("GET", uri("page?"), &owner)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "slug": slug, "status": "published" }),
        ))
        .await
        .unwrap();
    assert_eq!(response_json(res).await["page"]["status"].as_str(), Some("published"));

    // 权限：非成员读 404、写 403；公开库的 viewer 可读不可写。
    let stranger = TestUser::with_role("wiki-edit-stranger", "user");
    let res = app.clone().oneshot(authed_empty_request("GET", uri("revisions?"), &stranger)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "PUT",
            "/api/v1/knowledge/wiki/page",
            &stranger,
            serde_json::json!({ "kb_id": kb_id, "slug": slug, "content": "x" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    sqlx::query("UPDATE wiki_pages SET kb_id = ? WHERE kb_id = ?")
        .bind(public_kb)
        .bind(kb_id)
        .execute(&pool)
        .await
        .unwrap();
    let viewer = TestUser::with_role("wiki-edit-viewer", "user");
    let res = app
        .clone()
        .oneshot(authed_empty_request(
            "GET",
            format!("/api/v1/knowledge/wiki/revisions?kb_id={}&slug={}", public_kb, slug),
            &viewer,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "POST",
            "/api/v1/knowledge/wiki/revert",
            &viewer,
            serde_json::json!({ "kb_id": public_kb, "slug": slug, "version": 1 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    sqlx::query("UPDATE wiki_pages SET kb_id = ? WHERE kb_id = ?")
        .bind(kb_id)
        .bind(public_kb)
        .execute(&pool)
        .await
        .unwrap();

    // 删除：页面与历史一并清掉（外键级联）。
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "DELETE",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "slug": slug }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app.clone().oneshot(authed_empty_request("GET", uri("page?"), &owner)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let revisions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wiki_page_revisions").fetch_one(&pool).await.unwrap();
    assert_eq!(revisions, 0);
    let res = app
        .clone()
        .oneshot(authed_json_request(
            "DELETE",
            "/api/v1/knowledge/wiki/page",
            &owner,
            serde_json::json!({ "kb_id": kb_id, "slug": slug }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// utoipa 的注册错误只在构建文档时暴露，这里直接构建一次并检查 Wiki 写接口都在。
#[test]
fn openapi_documents_wiki_write_endpoints() {
    let doc = serde_json::to_value(htknow::api::openapi()).unwrap();
    let paths = &doc["paths"];
    for path in [
        "/api/v1/knowledge/wiki/pages",
        "/api/v1/knowledge/wiki/page",
        "/api/v1/knowledge/wiki/revisions",
        "/api/v1/knowledge/wiki/revision",
        "/api/v1/knowledge/wiki/revert",
        "/api/v1/knowledge/wiki/lint",
        "/api/v1/knowledge/wiki/rebuild-links",
    ] {
        assert!(paths.get(path).is_some(), "missing path {} in openapi doc", path);
    }
    for method in ["get", "put", "post", "delete"] {
        assert!(paths["/api/v1/knowledge/wiki/page"].get(method).is_some(), "missing {} /wiki/page", method);
    }
    for schema in ["WikiPageUpdateReq", "WikiRevisionResponse", "LintReport", "RevisionMeta"] {
        assert!(doc["components"]["schemas"].get(schema).is_some(), "missing schema {}", schema);
    }
}

#[tokio::test]
async fn wiki_file_progress_is_visible_in_lists_details_stats_and_retry() {
    let app = app().await;
    let pool = get_pool().await;
    let owner = TestUser::with_role("wiki-progress-owner", "user");
    let outsider = TestUser::with_role("wiki-progress-outsider", "user");
    let kb_id = insert_kb(&pool, &owner, "Wiki progress", "analysis", None, false).await;
    sqlx::query("UPDATE knowledge_bases SET wiki_config = '{\"enabled\":true}' WHERE id = ?")
        .bind(kb_id)
        .execute(&pool)
        .await
        .unwrap();
    let path = setup_env().data_dir.join(format!("wiki-progress-{}.txt", next_seq()));
    fs::write(&path, "A document whose original parsing has already completed.").unwrap();
    let file_id =
        insert_file(&pool, &owner, "wiki-progress.txt", &path, Some(kb_id), vec!["progress".into()], false).await;
    sqlx::query("UPDATE files SET status = 1 WHERE id = ?").bind(file_id).execute(&pool).await.unwrap();
    let slice_id = insert_slice(&pool, file_id, "original content must survive a Wiki retry").await;

    for url in [
        format!("/api/v1/knowledge/knowledge_base/{kb_id}/files"),
        format!("/api/v1/knowledge/knowledge_base/{kb_id}/files?tag=progress"),
        format!("/api/v1/knowledge/files/?kb_id={kb_id}"),
        format!("/api/v1/knowledge/files/?kb_id={kb_id}&tag=progress"),
    ] {
        let res = app.clone().oneshot(authed_empty_request("GET", &url, &owner)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{url}");
        let body = response_json(res).await;
        let file = body["items"].as_array().unwrap().iter().find(|f| f["id"] == file_id).unwrap();
        assert_eq!(file["status"], 1); // 原文仍可检索和预览。
        assert_eq!(file["processing_status"], 2);
        assert_eq!(file["wiki_status"], "pending");
    }
    let stats_url = format!("/api/v1/knowledge/files/stats?kb_id={kb_id}");
    let res = app.clone().oneshot(authed_empty_request("GET", &stats_url, &owner)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let stats = response_json(res).await;
    assert_eq!(stats["completed"], 0);
    assert_eq!(stats["processing"], 1);
    assert_eq!(stats["processing_files"][0]["id"], file_id);

    sqlx::query("INSERT INTO wiki_builds(file_id, status, error) VALUES(?, 'failed', 'upstream unavailable')")
        .bind(file_id)
        .execute(&pool)
        .await
        .unwrap();
    let detail_url = format!("/api/v1/knowledge/files/{file_id}");
    let res = app.clone().oneshot(authed_empty_request("GET", &detail_url, &owner)).await.unwrap();
    let file = response_json(res).await;
    assert_eq!(file["processing_status"], -1);
    assert_eq!(file["wiki_status"], "failed");
    assert_eq!(file["wiki_error"], "upstream unavailable");
    let denied = app.clone().oneshot(authed_empty_request("GET", &detail_url, &outsider)).await.unwrap();
    assert_ne!(denied.status(), StatusCode::OK);
    let request = serde_json::json!({"kb_id": kb_id, "include_descendants": true});
    let denied = app
        .clone()
        .oneshot(authed_json_request("POST", "/api/v1/knowledge/files/reparse-failed", &outsider, request.clone()))
        .await
        .unwrap();
    assert_ne!(denied.status(), StatusCode::OK);
    let res = app
        .clone()
        .oneshot(authed_json_request("POST", "/api/v1/knowledge/files/reparse-failed", &owner, request))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(response_json(res).await["file_count"], 1);
    let status: i32 =
        sqlx::query_scalar("SELECT status FROM files WHERE id = ?").bind(file_id).fetch_one(&pool).await.unwrap();
    assert_eq!(status, 1);
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM slices WHERE id = ?)")
        .bind(slice_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(exists);
    let res = app.clone().oneshot(authed_empty_request("GET", &detail_url, &owner)).await.unwrap();
    assert_eq!(response_json(res).await["wiki_status"], "pending");

    sqlx::query("UPDATE wiki_builds SET status = 'completed', page_count = 1, error = '' WHERE file_id = ?")
        .bind(file_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM wiki_tasks WHERE file_id = ? AND task_type = 'wiki:ingest'")
        .bind(file_id)
        .execute(&pool)
        .await
        .unwrap();
    let res = app.clone().oneshot(authed_empty_request("GET", &stats_url, &owner)).await.unwrap();
    let stats = response_json(res).await;
    assert_eq!(stats["completed"], 1);
    assert_eq!(stats["processing"], 0);
}
