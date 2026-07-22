# PostgreSQL Custom-Type Text Fallback Design

**Date:** 2026-07-22

## Goal

Fix issue #4171 so PostgreSQL extension and user-defined scalar values whose
binary representation DBX does not understand are rendered through the type's
server-provided text output instead of being mistaken for UTF-8 text.

The reported `bm25_catalog.bm25vector` values in
`public.bm25_test_documents` must display as values such as
`{1:2, 2:1, 10:1}`, without adding a decoder tied to one extension version.

## Root Cause

DBX normally executes PostgreSQL `SELECT` statements with the extended query
protocol. PostgreSQL therefore sends each result value in the type's binary
format.

For an unclassified type, `pg_fallback_value_to_json` eventually passes the raw
payload to `PgAnyString`. That adapter accepts every PostgreSQL type and treats
any byte sequence accepted by `std::str::from_utf8` as display text. A
`bm25vector` binary payload contains integer fields and zero bytes. Those bytes
are technically valid UTF-8 control characters, so the conversion succeeds and
the frontend renders replacement boxes instead of the type's textual value.

This is not a database encoding, stored-data, index, or frontend-font problem.
The live database reports UTF-8, and an explicit text conversion invokes the
type's output function and returns the expected `{term:frequency}` form.

## Requirements

- use PostgreSQL's own output function for unsupported extension and
  user-defined scalar types
- keep the binary prepared-query path for built-in and explicitly supported
  types
- decide the protocol before executing the statement, so a read query is not
  run twice
- preserve result column names and type names when the text path is selected
- retain current row limits, truncation behavior, timing, stale-statement
  recovery, and existing error-triggered text fallback
- avoid type-name checks or binary parsing specific to `bm25vector`
- avoid returning raw binary as hexadecimal when a meaningful server text form
  is available

## Chosen Approach

### Protocol decision from prepared metadata

DBX continues to prepare the statement first. Preparing exposes every output
column's PostgreSQL OID, type name, and the existing `PgColType`
classification, but does not execute the query.

After collecting that metadata, DBX chooses one of two paths:

- known built-in types and custom types with an explicit DBX binary decoder use
  the existing extended/binary query path
- an unclassified, non-system PostgreSQL type uses the existing simple-query
  text path for the whole result set

PostgreSQL allocates ordinary user and extension object OIDs from the normal
object range. The decision helper will combine that fact with
`PgColType::Other`; type name alone is insufficient because ordinary built-ins
such as `int4` and `varchar` also currently use the generic `Other` decoding
branch successfully.

The first unsupported custom output column selects text mode for the complete
query. PostgreSQL protocol result formats are query-wide in DBX's current
execution abstraction, and using one coherent path avoids merging rows from
separate executions.

### Prepared execution outcome

The prepared selector returns an internal outcome rather than fabricating a
PostgreSQL error:

- a completed `QueryResult` when binary mode is safe
- a text-fallback request carrying the prepared column type names when an
  unsupported custom type is present

`execute_select_query` handles that outcome before calling `query_raw`. Its
existing stale-cache retry and error-triggered fallback behavior remain in the
same orchestration layer.

### Text result metadata

The current `execute_select_text` implementation already receives the correct
text values through `SimpleQueryMessage::Row`, but returns an empty
`column_types` array. It will accept optional prepared type names for proactive
fallback and include them in `QueryResult` when their count matches the text
result's columns.

Existing error-triggered text fallback may not have trustworthy prepared
metadata, so it may continue with an empty type list. Mismatched metadata is
discarded rather than attached to the wrong columns.

### Observability

When DBX selects text mode proactively, it logs a concise reason including the
unsupported type name. It must not log row contents, connection credentials, or
other sensitive values.

## Compatibility And Error Handling

- `NULL` values remain JSON null in both protocols.
- Text-protocol values remain strings, matching the current fallback behavior.
- Queries containing only built-in or explicitly decoded types retain their
  current binary decoding and JSON shapes.
- Row limiting and truncation remain enforced while consuming text messages.
- A text-query failure is returned through the existing PostgreSQL error
  formatting path.
- Prepared-statement cache invalidation still retries once before applying the
  same protocol decision to fresh metadata.
- A PostgreSQL type whose binary format happens to be valid UTF-8 no longer
  qualifies as safe solely for that reason.

## Scope Boundaries

Included:

- normal extension or user-defined scalar output classified as
  `PgColType::Other`
- preservation of prepared column type names in proactive text fallback
- focused unit and PostgreSQL integration coverage

Not included:

- a `bm25vector` binary decoder
- changes to the grid or other frontend rendering code
- changing write, parameter-binding, export-stream, or metadata-query behavior
- redesigning generic custom-array decoding in this fix
- automatically casting or rewriting user SQL

## Testing

### Unit tests

- a normal-range OID classified as `Other` requires text protocol
- built-in OIDs classified as `Other`, including `int4` and `varchar`, remain
  on the binary path
- normal-range OIDs with explicit DBX handlers, such as vector or geometry,
  remain on the binary path
- any unsupported column is sufficient to select text mode for the query
- proactive text fallback preserves matching prepared type names and rejects
  mismatched metadata

### PostgreSQL integration test

Create a temporary user-defined type with distinct binary and text output,
query it through DBX, and assert that the returned cell contains the text form
and no NUL/control payload. Also include a built-in-only query to ensure the
normal binary path and JSON shapes are unchanged.

### Reported-data validation

Run DBX against the provided PostgreSQL instance and open
`public.bm25_test_documents`. Confirm all four `embedding` cells display their
`{term:frequency}` text and that ordinary columns in the same grid still render
normally.

The initial local baseline command could not run because `cargo` is not
installed or exposed in the current terminal. Implementation verification must
either locate the repository's configured Rust toolchain or report that test
execution remains unavailable; an unavailable command must not be described as
a passing test.

## Success Criteria

- issue #4171's four `bm25vector` values render as meaningful PostgreSQL text
- no extension-specific decoder or type-name allowlist is introduced
- supported result types keep their current binary-protocol behavior
- result metadata remains aligned with columns in proactive text fallback
- targeted tests and formatting checks pass in an available Rust toolchain
