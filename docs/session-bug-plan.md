# diff-viewer: показывает чужой репозиторий (reword-tui вместо agents-sync)

> ИСТОРИЧЕСКИЙ ДОКУМЕНТ: описывает ранний дизайн (per-tab touched, сигнатуры,
> соседи). Не соответствует коду с v0.5.0. Актуальная спецификация —
> `docs/AGENT-SCOPE-PLAN.md` (локальный, gitignored).

## Проблема
Первый воркспейс, первый таб: сессия редактирует `agents-sync`,
а diff-viewer показывает правки `reword-tui` (25 файлов, другая сессия).
Так быть не должно: scope репозиториев должен строиться по тому,
что редактирует **сессия**, а не по cwd пана / накопленному мусору.

Требование пользователя:
- набор репо — на сессию агента, не на таб;
- новая сессия — чистый набор; сессия закрылась/сменилась — данные удалены;
- репо попадает в scope по наблюдаемой активности сессии;
- внутри scope-репо показываем ВЕСЬ uncommitted (чьи правки — неважно,
  т.к. один репо из двух сессий без worktree редактировать нефиг);
- нет session id — fallback на pane_id.

## Причины (по коду)
1. `touched` копится **на таб и никогда не чистится**:
   `state.rs:48-59`, `note_touched` (`state.rs:77-88`).
   `state::remove` удаляет только toggle-файл (`main.rs:45`, `main.rs:181`),
   touched-файл переживает и вьюер, и сессии.
2. Scope = git-toplevel **cwd пана (anchor)** + весь накопленный touched;
   если cwd не репо — плюс все дочерние репо (`main.rs:50-69`,
   `ctx.rs:16-18`, `model.rs:36-82`). `DIFF_REPO` берётся из
   `workspace_cwd`/`focused_pane_cwd`, сессия не учитывается.
3. `agent_session.value` (напр. `ses_f601…` у opencode) есть в
   `herdr pane get`, но нигде не используется. Очистки при закрытии
   пана/таба/сессии нет.
4. Discovery слабый: только pane cwd + process cwds, и только пока
   открыт вьюер (`tty.rs:743-866`); `track-event` (`main.rs:73-106`)
   пишет лишь cwd пана. Правки файлов / edit-тулы не наблюдаются вообще.

## Живые данные (w4:t1, момент бага)
- `~/.local/state/herdr/plugins/odiumuniverse.diff-viewer/touched-w4_t1.json`
  = `[herdr-diff-viewer, /Users/universe, reword-tui]` (записан 13 сен).
- Пан `w4:p1` (opencode, «Анализ реализуемости AgentSync CLI»):
  cwd/foreground_cwd = `reword-tui`, session = `ses_f601…`.
- `agents-sync`: правки сегодня в 21:34 (текущая сессия);
  `reword-tui`: 24 файла, последние правки 02:47 (другая сессия).
- Вывод: вьюер показывает reword-tui, потому что scope =
  «cwd-репо + старый touched», а agents-sync туда никогда не попал.

## Дополнительные находки сессии
- Установленный плагин — GitHub-копия
  `odiumuniverse/herdr-diff-viewer@cee0203` (совпадает с HEAD локального репо);
  локальный репо не залинкован — для теста фикса нужен
  `herdr plugin uninstall` + `herdr plugin link`.
- В herdr есть события `pane.closed` / `pane.exited` / `tab.closed`
  (видны в EventData бинаря) — их можно добавить в `herdr-plugin.toml`,
  сейчас подписаны только `pane.created` / `pane.focused` /
  `pane.agent_status_changed`.
- Интеграции herdr↔агенты (`herdr-agent-state.js` у opencode) репортят
  только status + session id, путей файлов нет.
- Opencode считает сессию `ses_f601…` живущей в `reword-tui`
  (project directory), т.е. «где запущен агент» ≠ «где он правит».
- Точный канал для будущего: `pane.report_metadata` (tokens, как сейчас
  `quota_*`) — но это правка интеграции opencode/claude, отдельная задача.

## Черновик плана
1. State: `sessions/<pane>-<session>.json` в `HERDR_PLUGIN_STATE_DIR`
   (репо + baseline-сигнатуры + first_seen). Prune лениво по `pane list`
   + хуки `pane.closed`/`pane.exited`/`tab.closed` в `herdr-plugin.toml`.
   Legacy `touched-*.json` игнорировать/почистить.
2. Discovery: репо входит в scope сессии, если (а) cwd процесса из дерева
   сессии внутри него (сэмплинг 1с во вьюере + на `agent_status_changed`)
   и/или (б) сигнатура рабочего дерева изменилась при живой сессии
   (кандидаты ограничены). Убрать eager `child_repos` и anchor-first.
3. Viewer: scope = union живых сессий агентских панов таба; пустой scope —
   валидное состояние («0 files», без ошибки); «watching N» = репо в scope.
4. README + тесты (prune state, сборка scope, track-event).

## Ответы пользователя на вопросы
1. Правило scope — **только по активности** (Recommended).
2. Discovery reach — не понял вопрос («мы же слушаем bus у herdr просто»).
3. Session end — **ленивый prune + события** (Recommended).
4. Точность — «опять же, вроде как слушаем просто herdr».

## Открытый вопрос (не закрыт ответами)
Что именно понимается под «слушаем bus herdr» для детекта правок:
сейчас у плагина только хуки pane.created/focused/agent_status_changed
+ опрос `herdr pane get/list/process-info`, событий уровня «файл изменён»
в шине нет. Подтвердить: полагаемся на cwd процессов + поллинг сигнатур
рабочего дерева (вариант «процессы + соседние репо»), или только на
процессы без скана ФС.
