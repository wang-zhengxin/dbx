# PostgreSQL 自定义类型文本回退实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 当 PostgreSQL 查询包含 DBX 不支持其二进制格式的扩展或用户自定义标量类型时，在执行前切换到服务器文本协议，并保留查询列类型元数据。

**架构：** `execute_select_prepared` 仍先准备语句并检查输出列的 OID 与现有 `PgColType` 分类；普通对象 OID 且分类为 `Other` 的列返回内部 `TextFallback` 结果，不调用 `query_raw`。`execute_select_query` 消费该结果并调用现有 `simple_query` 路径，同时把准备阶段取得的列类型名称传给文本结果。

**技术栈：** Rust、Tokio、deadpool-postgres、tokio-postgres、serde_json、内置单元测试、Docker PostgreSQL 集成测试

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

- [ ] **步骤 4：编写真实 PostgreSQL 回归测试并验证 RED**

在现有 Docker PostgreSQL 测试附近加入：

```rust
#[tokio::test]
async fn postgres_custom_composite_result_uses_server_text_output() {
    let Some(container) = start_docker_postgres().await else {
        return;
    };

    let pool = connect(&container.url(), Duration::from_secs(5)).await.expect("connect postgres");
    let schema = format!("dbx_custom_text_{}", std::process::id());
    let schema_ident = pg_quote_ident(&schema);
    let payload_type = format!("{schema_ident}.payload");

    execute_query(&pool, &format!("CREATE SCHEMA {schema_ident}"))
        .await
        .expect("create schema");
    execute_query(&pool, &format!("CREATE TYPE {payload_type} AS (id integer, label text)"))
        .await
        .expect("create composite type");

    let custom = execute_query(
        &pool,
        &format!("SELECT ROW(7, 'alpha')::{payload_type} AS payload, 42::int4 AS id"),
    )
    .await
    .expect("query custom composite value");

    assert_eq!(custom.columns, vec!["payload", "id"]);
    assert_eq!(custom.column_types, vec!["payload", "int4"]);
    assert_eq!(custom.rows[0][0], serde_json::Value::String("(7,alpha)".to_string()));
    assert_eq!(custom.rows[0][1], serde_json::Value::String("42".to_string()));
    assert!(!custom.rows[0][0].as_str().unwrap().chars().any(char::is_control));

    let builtin = execute_query(&pool, "SELECT 42::int4 AS id").await.expect("query built-in value");
    assert_eq!(builtin.column_types, vec!["int4"]);
    assert_eq!(builtin.rows[0][0], serde_json::Value::Number(42.into()));
}
```

运行：

```bash
cargo test -p dbx-core postgres_custom_composite_result_uses_server_text_output --lib -- --nocapture
```

预期：Docker 可用时 FAIL，`payload` 单元格不是 `(7,alpha)` 或仍含二进制控制内容。Docker 不可用时先继续单元测试实现，最终必须使用报告数据库补足端到端验收。

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
cargo test -p dbx-core postgres_custom_composite_result_uses_server_text_output --lib -- --nocapture
```

预期：所有目标单元测试通过；Docker 可用时集成测试通过，并断言自定义复合类型为服务器文本 `(7,alpha)`、纯内置查询仍返回数字 JSON。

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
```

预期：全部通过；Docker 不可用导致的辅助测试跳过需在结果中单独说明。

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
