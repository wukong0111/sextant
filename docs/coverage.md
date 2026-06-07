# Coverage map — `SPEC.md` §17 → tests

The binding between the **agnostic** acceptance criteria (`SPEC.md` §17) and the
**concrete** tests of this Rust implementation. `SPEC.md` stays free of test
names on purpose; this mapping is per-implementation and lives here.

**Tiers** (strongest first):

- **E2E** — PTY tests in `crates/sextant-cli/tests/e2e.rs`: drive the real
  binary through a pseudo-terminal. Verify the integrated, observable behavior.
- **APP** — `TestBackend` tests in `crates/sextant-ui/src/lib.rs`: drive the
  `App` state machine + render, no real TTY. Verify integrated UI behavior.
- **UNIT** — logic-level tests in the service crates.
- **—** — no test (gap).

| # | `SPEC.md` §17 scenario | Tier | Tests |
|---|------------------------|------|-------|
| 1 | Arranque y salida limpia | **E2E** | `boots_renders_connection_and_quits_cleanly`; app: `app_default_state`, `ctrl_q_sets_should_quit`, `ctrl_q_with_dirty_buffer_prompts` |
| 2 | Conectar y consultar | **E2E** | `editor_query_is_recorded_in_history`; app: `editor_toggle_with_space_e`, `grid_renders_when_result_present`, `status_line_shows_row_count_and_duration` |
| 3 | El historial registra toda ejecución | **E2E** | `editor_query_is_recorded_in_history`; unit: `query_history_records_and_lists_newest_first` (incl. fila con error), `recent_queries_respects_limit` |
| 4 | Browse de tabla (`SELECT * … LIMIT 500`) | **E2E+APP** | `browse_table_renders_rows_with_limit` (flujo árbol→grid); app: `browse_table_builds_select_with_limit_500` (contrato SQL: `LIMIT 500` + quoting) |
| 5 | Edición del grid + concurrencia optimista | **APP+UNIT** | widget: `editing_a_cell_generates_update`, `adding_a_row_generates_insert`, `marking_a_row_generates_delete`, `editing_pk_uses_original_value_in_where`, `discard_clears_all_pending`; app: `ctrl_s_opens_commit_modal_and_esc_cancels`; unit `sql.rs`: `update_statement_by_pk`, `where_match_uses_is_null_for_null_values`, `update_with_null_original_in_where`. **Sin PTY** para el commit completo |
| 6 | Grid de solo lectura | **UNIT** | `read_only_without_context_or_pk` |
| 7 | Transacción de sesión psql-style | **UNIT** | `session_transaction_commits`, `session_transaction_rolls_back` (ve cambios no confirmados), `classifies_txn_control`. **Sin PTY** |
| 8 | Guardia de operaciones destructivas | **APP+UNIT** | app: `dangerous_editor_sql_requires_confirmation`, `safe_editor_sql_runs_without_prompt`; unit: `dangerous_flags_unguarded_dml_and_ddl`, `dangerous_allows_guarded_dml_and_reads` |
| 9 | Resolución de credenciales | **APP+UNIT** | unit `sextant-config`: `connection_password_from_env`, `resolve_password_prefers_keyring_over_env` (orden), `resolve_password_falls_back_to_env`, `resolve_password_prompts_when_keyring_key_but_no_secret`, `resolve_password_sqlite_never_prompts`, `resolve_password_tcp_without_keyring_key_connects_passwordless`; app (doble en memoria `InMemoryStore`): `password_prompt_captures_input_and_cancels`, `start_connection_consults_store_then_prompts`, `persist_pending_credential_saves_on_match_and_clears`, `persist_pending_credential_ignores_other_connections`, `failed_connection_discards_pending_credential`. Costuras en ADR-0005. **Sin PTY**; keyring real y connect real → QA manual |
| 10 | Export | **E2E** | `exports_result_set_to_csv_file`; unit `export.rs`: `csv_has_header_and_empty_null`, `json_is_array_of_objects_with_typed_values`, `sql_emits_insert_per_row_with_typed_literals`, … |
| 11 | Import | **E2E** | `imports_csv_into_selected_table`; unit `import.rs`: `csv_parses_header_and_rows`, `preview_counts_type_issues`, `build_inserts_uses_only_mapped_columns_and_typed_literals`, … |
| 12 | Recuperación ante caída (swap) | **APP+UNIT** | app: `recovery_restore_loads_buffers_into_editor`, `recovery_discard_clears_prompt_without_opening_editor`; unit `swap.rs`: `round_trips_through_json`, `parse_rejects_garbage`, `session_path_is_in_swap_dir`. **Sin PTY** |
| 13 | Remapeo de teclas | **UNIT** | `user_binding_overrides_default_chord`, `user_can_add_alternate_chord`, `unknown_action_name_is_skipped` |
| 14 | Autocomplete de tablas y columnas | **E2E** | `autocomplete_inserts_table_name`; unit: `after_from_filters_by_prefix`, `dotted_table_offers_columns`, `ctrl_space_triggers_table_completion`, `enter_accepts_completion_and_replaces_prefix` |
| 15 | Schema viewer (columnas en árbol) | **E2E** | `schema_viewer_shows_columns_in_tree`; unit: `expand_table_shows_columns` |

## Resumen

- **PTY end-to-end**: escenarios **1, 2, 3, 4, 10, 11, 14, 15**.
- **Verificados a nivel app/unit** (comportamiento integrado, sin TTY real):
  **5, 6, 7, 8, 9, 12**.
- **Huecos reales**: ninguno pendiente.

## Qué NO pueden cubrir los tests (verificación manual recurrente)

No es una lista de tareas: es el **catálogo de aspectos que solo un humano puede
verificar**, a revisar **cada vez** que se toca el área relacionada (marcar algo
como "hecho" no garantiza que siga funcionando tras un cambio futuro). El *qué*
funcional está en `SPEC.md` §17; el *setup* en las skills `db-setup` /
`connect-tui`. Aquí solo lo no automatizable:

- **Color y resaltado reales** — `TestBackend` valida el *estilo* asignado, no su
  render en una TTY: celda modificada resaltada, fila nueva en verde, fila
  borrada tachada en rojo (§17.5); `txn: ACTIVE` en ámbar (§17.7); indicador 🔒
  de solo lectura (§17.6); spinner de trabajo asíncrono; tema aplicado de forma
  coherente (bordes, tabs, popup de autocomplete).
- **Feel en TTY real** — timing de pulsaciones, secuencias de escape (un `Esc`
  solo vs. inicio de secuencia), ausencia de glitches de render, layout y status
  line correctos en ~80 columnas.
- **Multi-driver PG / MySQL** — los e2e solo cubren SQLite; el comportamiento
  contra PostgreSQL y MySQL (conexión, introspección de índices/FKs, browse,
  edición + commit) solo se ejercita a mano con los contenedores Docker.
- **Keyring real y connect real (§17.9)** — los tests usan un `CredentialStore`
  en memoria y no construyen una conexión real (ver ADR-0005). Queda manual:
  conectar a PG/MySQL con `keyring_key` sin secreto → prompt enmascarado → tras
  conectar con éxito, la contraseña queda guardada en el keyring del SO y no se
  reescribe `connections.toml`.

## Disciplina

Toda feature nueva o cambio de comportamiento añade su criterio en `SPEC.md` §17
y su(s) test(s) aquí (ver `docs/documentation-guide.md`). Un escenario sin fila
en esta tabla, o con tier **—**, es deuda de test explícita. Lo no automatizable
(color, feel en TTY, multi-driver) pertenece a la sección anterior.
