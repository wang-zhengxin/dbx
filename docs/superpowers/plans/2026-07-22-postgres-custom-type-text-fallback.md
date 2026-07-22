# PostgreSQL 自定义类型文本回退实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 当 PostgreSQL 查询包含 DBX 不支持其二进制格式的扩展或用户自定义标量类型时，在执行前切换到服务器文本协议，并保留查询列类型元数据。

**架构：** `execute_select_prepared` 仍先准备语句并检查输出列的 OID 与现有 `PgColType` 分类；普通对象 OID 且分类为 `Other` 的列会先用非缓存 prepare 复核元数据，再返回内部 `TextFallback` 结果，不调用 `query_raw`。`execute_select_query` 消费该结果并调用 `simple_query_raw` 文本流，同时把准备阶段取得的列类型名称传给文本结果。

**技术栈：** Rust、Tokio、deadpool-postgres、tokio-postgres、serde_json、内置单元测试、外部 PostgreSQL 18 ignored 集成测试

---

## 文件结构

- 修改：`crates/dbx-core/src/db/postgres.rs` — 自定义类型协议决策、准备查询结果编排、文本结果元数据以及相关单元/集成测试。
- 已有规格：`docs/superpowers/specs/2026-07-22-postgres-custom-type-text-fallback-design.md` — 行为边界和验收标准；实现不得加入 `bm25vector` 专用解码器。

此修复沿用 PostgreSQL 适配器现有的单文件组织，不拆分模块，也不修改前端、导出流或参数绑定逻辑。

### 任务 1：用测试锁定通用协议决策

**文件：**
- 修改：`crates/dbx-core/src/db/postgres.rs:320-410`
- 测试：`crates/dbx-core/src/db/postgres.rs:3531-3590`

- [ ] **步骤 1：编写失败的协议决策单元测试**

在 `tests` 模块中加入以下测试。它们必须先因常量和函数尚不存在而编译失败：

```rust
#[test]
fn postgres_custom_other_type_requires_text_protocol() {
    assert!(pg_type_requires_text_protocol(POSTGRES_FIRST_NORMAL_OBJECT_ID, PgColType::Other));
    assert!(pg_type_requires_text_protocol(98_765, PgColType::Other));
}

#[test]
fn postgres_builtin_or_supported_type_keeps_binary_protocol() {
    assert!(!pg_type_requires_text_protocol(Type::INT4.oid(), PgColType::Other));
    assert!(!pg_type_requires_text_protocol(Type::VARCHAR.oid(), PgColType::Other));
    assert!(!pg_type_requires_text_protocol(98_765, PgColType::Vector));
    assert!(!pg_type_requires_text_protocol(98_765, PgColType::Geometry));
}

#[test]
fn postgres_query_uses_text_when_any_output_type_is_unsupported() {
    let columns = [
        (Type::INT4.oid(), PgColType::Other),
        (98_765, PgColType::Other),
        (Type::TEXT.oid(), PgColType::Other),
    ];

    assert!(columns.into_iter().any(|(oid, col_type)| pg_type_requires_text_protocol(oid, col_type)));
}
```

- [ ] **步骤 2：运行测试并验证 RED**

运行：

```bash
cargo test -p dbx-core postgres_custom_other_type_requires_text_protocol --lib
```

预期：编译失败，错误指出 `POSTGRES_FIRST_NORMAL_OBJECT_ID` 和 `pg_type_requires_text_protocol` 不存在。若当前终端仍找不到 `cargo`，先定位项目配置的 Rust 工具链；不可把命令不可用记录为测试失败或通过。

- [ ] **步骤 3：实现最小协议决策函数**

紧邻 `PgColType` 定义加入：

```rust
const POSTGRES_FIRST_NORMAL_OBJECT_ID: u32 = 16_384;

fn pg_type_requires_text_protocol(oid: u32, col_type: PgColType) -> bool {
    oid >= POSTGRES_FIRST_NORMAL_OBJECT_ID && matches!(col_type, PgColType::Other)
}
```

该函数只把普通对象范围内、且 DBX 没有显式二进制处理器的类型视为不安全。内置 `int4`、`varchar` 的 OID 低于边界；扩展 `vector` 和 `geometry` 虽通常有普通对象 OID，但已有显式分类。

- [ ] **步骤 4：运行目标单元测试并验证 GREEN**

运行：

```bash
cargo test -p dbx-core postgres_custom_other_type_requires_text_protocol --lib
cargo test -p dbx-core postgres_builtin_or_supported_type_keeps_binary_protocol --lib
cargo test -p dbx-core postgres_query_uses_text_when_any_output_type_is_unsupported --lib
```

预期：三个测试全部通过，且没有编译警告。

- [ ] **步骤 5：提交协议决策**

```bash
git add crates/dbx-core/src/db/postgres.rs
git commit -m "test: define PostgreSQL custom type fallback boundary"
```

### 任务 2：在执行前切换文本协议并保留元数据

**文件：**
- 修改：`crates/dbx-core/src/db/postgres.rs:690-832`
- 测试：`crates/dbx-core/src/db/postgres.rs:3531-3620,4220-4290`

- [ ] **步骤 1：编写失败的文本元数据单元测试**

先在生产模块中声明但不实现调用的目标函数名，然后在 `tests` 模块加入：

```rust
#[test]
fn postgres_text_fallback_keeps_matching_prepared_column_types() {
    let columns = vec!["payload".to_string(), "id".to_string()];
    let types = vec!["payload_type".to_string(), "int4".to_string()];

    assert_eq!(matching_pg_text_column_types(&columns, Some(types.clone())), types);
}

#[test]
fn postgres_text_fallback_discards_misaligned_column_types() {
    let columns = vec!["payload".to_string(), "id".to_string()];
    let types = vec!["payload_type".to_string()];

    assert!(matching_pg_text_column_types(&columns, Some(types)).is_empty());
    assert!(matching_pg_text_column_types(&columns, None).is_empty());
}
```

- [ ] **步骤 2：运行元数据测试并验证 RED**

运行：

```bash
cargo test -p dbx-core postgres_text_fallback_ --lib
```

预期：编译失败，错误指出 `matching_pg_text_column_types` 不存在。

- [ ] **步骤 3：实现文本元数据校验函数并验证 GREEN**

在 `execute_select_text` 前加入：

```rust
fn matching_pg_text_column_types(columns: &[String], prepared: Option<Vec<String>>) -> Vec<String> {
    prepared.filter(|types| types.len() == columns.len()).unwrap_or_default()
}
```

此时先只加入 `matching_pg_text_column_types`，尚不修改 `execute_select_text` 调用链。运行：

```bash
cargo test -p dbx-core postgres_text_fallback_ --lib
```

预期：两个元数据测试通过。

- [ ] **步骤 4：编写 PostgreSQL 18 回归测试并验证 RED**

测试使用调用方通过 `DBX_TEST_POSTGRES_URL` 注入的可写 PostgreSQL 18 实例，不在源码、命令或日志中保存连接地址和凭据。每个测试先执行 `SHOW server_version_num` 并确认版本在 `180000..190000`，使用 UUID 创建唯一 schema，并在 unwrap/断言前执行 `DROP SCHEMA ... CASCADE`。

加入三个 `#[ignore = "requires DBX_TEST_POSTGRES_URL pointing at a writable PostgreSQL 18 database"]` 回归测试：

- `postgres_custom_composite_result_uses_server_text_output`：自定义 composite 通过服务器文本输出返回 `(7,alpha)`，同查询内建值为文本，纯内建查询仍为 JSON Number。
- `postgres_custom_type_fallback_refreshes_stale_cached_metadata`：pool A 缓存 custom view 元数据，pool B 将同名 view 改成 `int4`，pool A 重查必须使用 fresh metadata 并返回 JSON Number。
- `postgres_text_fallback_stops_before_late_row_error_at_limit`：custom 查询在第 N+1 行直接报错，`row_limit=1` 必须返回首行并标记 truncated；同一 client 随后的内建查询必须成功。

核心测试结构如下：

```rust
#[tokio::test]
#[ignore = "requires DBX_TEST_POSTGRES_URL pointing at a writable PostgreSQL 18 database"]
async fn postgres_custom_composite_result_uses_server_text_output() {
    let url = std::env::var("DBX_TEST_POSTGRES_URL").expect("DBX_TEST_POSTGRES_URL");
    let pool = connect(&url, Duration::from_secs(5)).await.expect("connect postgres");
    assert_postgres_18(&pool).await;
    let schema = format!("dbx_custom_text_{}", uuid::Uuid::new_v4().simple());
    let schema_ident = pg_quote_ident(&schema);
    let payload_type = format!("{schema_ident}.payload");

    execute_query(&pool, &format!("CREATE SCHEMA {schema_ident}")).await.expect("create schema");
    let exercise = async { /* create type and run custom/builtin queries */ }.await;
    let cleanup = execute_query(&pool, &format!("DROP SCHEMA {schema_ident} CASCADE")).await;
    cleanup.expect("drop schema");
    let (custom, builtin) = exercise.expect("exercise custom composite fallback");
    // Assertions run only after cleanup.
}
```

运行：

```bash
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_custom_composite_result_uses_server_text_output --lib -- --ignored --nocapture
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_custom_type_fallback_refreshes_stale_cached_metadata --lib -- --ignored --nocapture
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_text_fallback_stops_before_late_row_error_at_limit --lib -- --ignored --nocapture
```

预期 RED：原始实现分别暴露自定义值的二进制控制内容、陈旧 custom metadata，以及第 N+1 行范围外错误。不得把缺少环境变量、连接失败或版本不符当作 RED。

- [ ] **步骤 5：实现准备查询结果类型**

在 `execute_select_prepared` 前定义：

```rust
enum PreparedSelectOutcome {
    Complete(QueryResult),
    TextFallback { column_types: Vec<String>, unsupported_type: String },
}
```

把 `execute_select_prepared` 返回类型改为：

```rust
) -> Result<PreparedSelectOutcome, tokio_postgres::Error> {
```

取得 `column_types` 和 `column_classes` 后、创建 `query_raw` 前加入：

```rust
if let Some(unsupported_type) = stmt.columns().iter().zip(&column_classes).find_map(|(column, col_type)| {
    let pg_type = column.type_();
    pg_type_requires_text_protocol(pg_type.oid(), *col_type).then(|| pg_type.name().to_string())
}) {
    return Ok(PreparedSelectOutcome::TextFallback { column_types, unsupported_type });
}
```

cached prepare 首次命中自定义类型候选时，必须在执行 SQL 前调用一次 `client.prepare(sql)` 做非缓存复核，并以 fresh statement 的 columns、types 和 classes 替换缓存元数据。fresh 仍为不支持的 custom 类型时返回 `TextFallback`；fresh 已为 built-in 时使用 fresh statement 进入 `query_raw`。这避免陈旧 custom metadata 绕过 stale-plan 错误，同时保证 SQL 只执行一次。

原函数末尾返回改为：

```rust
Ok(PreparedSelectOutcome::Complete(QueryResult {
    columns,
    column_types,
    column_sortables: Vec::new(),
    rows: result_rows,
    affected_rows: 0,
    execution_time_ms: start.elapsed().as_millis(),
    truncated,
    session_id: None,
    has_more: false,
}))
```

- [ ] **步骤 6：扩展文本查询并统一消费准备查询结果**

将 `execute_select_text` 的签名扩展为：

```rust
async fn execute_select_text(
    client: &deadpool_postgres::Client,
    sql: &str,
    start: Instant,
    row_limit: usize,
    prepared_column_types: Option<Vec<String>>,
) -> Result<QueryResult, String>
```

文本路径使用 `client.simple_query_raw(sql)` 并逐消息消费。在 `Ok(Row)` 到达且已达 row limit 时标记 `truncated` 后退出；若第 N+1 条消息直接是 `Err`，同样标记 `truncated` 后退出，只有未达 limit 的错误才向上传播。`CommandComplete` 不得让“恰好等于 limit”被误标为 truncated。

在构造 `QueryResult` 前计算并返回类型列表：

```rust
let column_types = matching_pg_text_column_types(&columns, prepared_column_types);

Ok(QueryResult {
    columns,
    column_types,
    column_sortables: Vec::new(),
    rows: result_rows,
    affected_rows: 0,
    execution_time_ms: start.elapsed().as_millis(),
    truncated,
    session_id: None,
    has_more: false,
})
```

已有错误触发的调用传 `None`，只有准备阶段主动选择文本协议时传 `Some(column_types)`。

在 `execute_select_query` 前加入：

```rust
async fn finish_prepared_select(
    client: &deadpool_postgres::Client,
    sql: &str,
    start: Instant,
    row_limit: usize,
    outcome: PreparedSelectOutcome,
) -> Result<QueryResult, String> {
    match outcome {
        PreparedSelectOutcome::Complete(result) => Ok(result),
        PreparedSelectOutcome::TextFallback { column_types, unsupported_type } => {
            log::info!(
                "[postgres][select:text_fallback] unsupported_type={} switching_to=simple_query",
                unsupported_type
            );
            execute_select_text(client, sql, start, row_limit, Some(column_types)).await
        }
    }
}
```

将 `execute_select_query` 两处成功分支都交给 `finish_prepared_select`。两处 `should_retry_postgres_text_query` 分支调用：

```rust
execute_select_text(client, sql, start, row_limit, None).await
```

这样协议决策发生在 `query_raw` 之前，查询不会执行两次；刷新陈旧语句后仍会重新检查类型。

- [ ] **步骤 7：运行单元和集成测试并验证 GREEN**

运行：

```bash
cargo test -p dbx-core postgres_text_fallback_ --lib
cargo test -p dbx-core postgres_custom_other_type_requires_text_protocol --lib
cargo test -p dbx-core postgres_builtin_or_supported_type_keeps_binary_protocol --lib
cargo test -p dbx-core postgres_query_uses_text_when_any_output_type_is_unsupported --lib
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_custom_composite_result_uses_server_text_output --lib -- --ignored --nocapture
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_custom_type_fallback_refreshes_stale_cached_metadata --lib -- --ignored --nocapture
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_text_fallback_stops_before_late_row_error_at_limit --lib -- --ignored --nocapture
```

预期：所有目标单元测试通过；三个 PostgreSQL 18 ignored 测试通过，并分别覆盖服务器文本输出、陈旧元数据刷新和 N+1 范围外错误；纯内置查询仍返回数字 JSON。

- [ ] **步骤 8：提交协议编排和回归测试**

```bash
git add crates/dbx-core/src/db/postgres.rs
git commit -m "fix: fall back to text for PostgreSQL custom types"
```

### 任务 3：完整验证与报告数据验收

**文件：**
- 验证：`crates/dbx-core/src/db/postgres.rs`
- 验证：`public.bm25_test_documents`（用户提供的 PostgreSQL 实例，只读查询）

- [ ] **步骤 1：格式化并检查差异**

运行：

```bash
cargo fmt --all -- --check
git diff origin/main --check
```

预期：两个命令均退出码 0。

- [ ] **步骤 2：运行 PostgreSQL 适配器测试**

运行：

```bash
cargo test -p dbx-core db::postgres::tests --lib
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_custom_composite_result_uses_server_text_output --lib -- --ignored --nocapture
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_custom_type_fallback_refreshes_stale_cached_metadata --lib -- --ignored --nocapture
env DBX_TEST_POSTGRES_URL="$DBX_TEST_POSTGRES_URL" cargo test -p dbx-core postgres_text_fallback_stops_before_late_row_error_at_limit --lib -- --ignored --nocapture
```

预期：单元测试和三个 PostgreSQL 18 ignored 测试全部通过。若 `DBX_TEST_POSTGRES_URL` 未提供，则必须明确报告集成测试未运行，不得把 ignored 状态视为通过。

- [ ] **步骤 3：运行 crate 检查**

运行：

```bash
cargo check -p dbx-core
```

预期：退出码 0，没有新增警告。

- [ ] **步骤 4：验证报告表**

使用修复后的本地 DBX 打开 `public.bm25_test_documents`，确认四行 `embedding` 分别显示服务器返回的 `{term:frequency}` 文本，不含方框或控制字符。确认同一结果中的 `id`、`title`、`content` 仍能正常阅读。

- [ ] **步骤 5：复查提交和工作区**

运行：

```bash
git status --short --branch
git log --oneline origin/main..HEAD
```

预期：工作区干净；提交列表只包含设计、实现计划、协议决策、修复和回归测试，不包含凭据、构建产物或临时文件。
