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
| 4 | Browse de tabla (`SELECT * … LIMIT 500`) | **—** | **gap**: ningún test afirma el contrato `LIMIT 500` ni el flujo browse-desde-árbol |
| 5 | Edición del grid + concurrencia optimista | **APP+UNIT** | widget: `editing_a_cell_generates_update`, `adding_a_row_generates_insert`, `marking_a_row_generates_delete`, `editing_pk_uses_original_value_in_where`, `discard_clears_all_pending`; app: `ctrl_s_opens_commit_modal_and_esc_cancels`; unit `sql.rs`: `update_statement_by_pk`, `where_match_uses_is_null_for_null_values`, `update_with_null_original_in_where`. **Sin PTY** para el commit completo |
| 6 | Grid de solo lectura | **UNIT** | `read_only_without_context_or_pk` |
| 7 | Transacción de sesión psql-style | **UNIT** | `session_transaction_commits`, `session_transaction_rolls_back` (ve cambios no confirmados), `classifies_txn_control`. **Sin PTY** |
| 8 | Guardia de operaciones destructivas | **APP+UNIT** | app: `dangerous_editor_sql_requires_confirmation`, `safe_editor_sql_runs_without_prompt`; unit: `dangerous_flags_unguarded_dml_and_ddl`, `dangerous_allows_guarded_dml_and_reads` |
| 9 | Resolución de credenciales | **APP+UNIT (parcial)** | unit: `connection_password_from_env` (solo la rama env); app: `password_prompt_captures_input_and_cancels` (UI del prompt). **gaps**: lookup/escritura en keyring, orden de la cascada, guardar-tras-conectar |
| 10 | Export | **E2E** | `exports_result_set_to_csv_file`; unit `export.rs`: `csv_has_header_and_empty_null`, `json_is_array_of_objects_with_typed_values`, `sql_emits_insert_per_row_with_typed_literals`, … |
| 11 | Import | **E2E** | `imports_csv_into_selected_table`; unit `import.rs`: `csv_parses_header_and_rows`, `preview_counts_type_issues`, `build_inserts_uses_only_mapped_columns_and_typed_literals`, … |
| 12 | Recuperación ante caída (swap) | **APP+UNIT** | app: `recovery_restore_loads_buffers_into_editor`, `recovery_discard_clears_prompt_without_opening_editor`; unit `swap.rs`: `round_trips_through_json`, `parse_rejects_garbage`, `session_path_is_in_swap_dir`. **Sin PTY** |
| 13 | Remapeo de teclas | **UNIT** | `user_binding_overrides_default_chord`, `user_can_add_alternate_chord`, `unknown_action_name_is_skipped` |

## Resumen

- **PTY end-to-end**: escenarios **1, 2, 3, 10, 11**.
- **Verificados a nivel app/unit** (comportamiento integrado, sin TTY real):
  **5, 6, 7, 8, 12**.
- **Huecos reales** (a cerrar):
  - **#4 Browse** — añadir un e2e que haga *Activate* sobre una tabla y afirme el
    resultado de `SELECT * FROM <t> LIMIT 500`; barato.
  - **#9 Credenciales** — falta cubrir el lookup/guardado en keyring, el **orden**
    de la cascada (keyring → env → prompt) y el guardar-tras-conectar.

## Disciplina

Toda feature nueva o cambio de comportamiento añade su criterio en `SPEC.md` §17
y su(s) test(s) aquí (ver `docs/documentation-guide.md`). Un escenario sin fila
en esta tabla, o con tier **—**, es deuda de test explícita.
