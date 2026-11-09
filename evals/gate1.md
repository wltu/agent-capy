# Gate 1 eval: capy on HearthstoneGo

▎ HearthstoneGo summary (ground truth)
▎
▎ HearthstoneGo is an unofficial Go library that wraps Blizzard's Hearthstone API. Callers create a client with NewAPI(location, clientID, clientSecret). It gets an OAuth token from Blizzard, picks the regional endpoint and locale from the location name, and loads metadata (sets, classes, rarities, keywords and more) at setup time. Through that client you can fetch one card by ID (constructed or Battlegrounds), fetch a filtered collection of cards (a Begin…/Set…/End… builder: class, mana cost, rarity, keyword and so on), fetch one card back or a filtered collection of card backs, look up a deck by deck code, and download card images.
▎
▎ Easy-to-get-wrong detail: despite its name, cmd/api holds the library, not a binary, and the tests live apart in internal/test. NewAPI fails two different ways: an unknown location prints the supported list and returns (nil, false), but missing credentials panic (hearstone_api.go:112-115). The design doc says setup "returns false" on auth failure, which is wrong for the credentials case.

Tests
$ grep -c "func Test" /Users/wltu/workspace/HearthstoneGo/internal/test/*.go
/Users/wltu/workspace/HearthstoneGo/internal/test/card_back_collection_test.go:3
/Users/wltu/workspace/HearthstoneGo/internal/test/card_back_test.go:2
/Users/wltu/workspace/HearthstoneGo/internal/test/card_collection_test.go:5
/Users/wltu/workspace/HearthstoneGo/internal/test/card_test.go:6
/Users/wltu/workspace/HearthstoneGo/internal/test/deck_test.go:1
/Users/wltu/workspace/HearthstoneGo/internal/test/hearthstone_api_test.go:2

expect 0 to run since the setup will fail.

Run log
Target: /Users/wltu/workspace/HearthstoneGo · model qwen3.8-27b

| #   | Task                 | Model calls | Tool calls                     | Tokens in/out | Wall   | Correct?             | Odd       |
| --- | -------------------- | ----------- | ------------------------------ | ------------- | ------ | -------------------- | --------- |
| 1   | summary              | 4           | 5 (3 bash, 2 read)             | 6979 / 644    | n/a    | partial              | see below |
| 2   | summary              | 3           | 4 (2 bash, 2 read)             | 4072 / 473    | 57.5s  | partial              | see below |
| 3   | summary              | 3           | 3 (2 bash, 1 read)             | 5082 / 476    | 51.1s  | partial              | see below |
| 4   | summary (new prompt) | 4           | 4 (2 bash, 2 read)             | 7146 / 623    | 69.2s  | yes, trap missed     | see below |
| 5   | summary (new prompt) | 5           | 7 (2 bash, 5 read)             | 17420 / 851   | 99.6s  | yes, trap missed     | see below |
| 6   | summary (new prompt) | 4           | 6 (2 bash, 4 read)             | 9642 / 841    | 100.4s | yes, trap missed     | see below |
| 7   | tests                | 5           | 6 (3 bash, 1 declined; 3 read) | 6574 / 736    | 106.2s | yes, one wrong claim | see below |
| 8   | tests                | 7           | 7 (5 bash, 2 read)             | 14687 / 1309  | 153.7s | yes, over-explored   | see below |
| 9   | tests                | 5           | 4 (2 bash, 2 read)             | 9155 / 779    | 85.4s  | yes, clean           | see below |

Run 1 notes

- Read only README.md and go.mod. Never opened a .go file.
- Said "Let me take a quick look at the main file", then answered without reading it.
- Right: library (not a binary) in cmd/api, NewAPI + creds, card / BG card / filtered collection / card back / deck, Go 1.14, MIT, CI.
- Wrong: "images package fetches and renders images" (images/ is just a .gitignore; image code is in cmd/api/request.go). "data edition" made up. "client.NewAPI" (it's api.NewAPI).
- Missed: metadata loaded at setup. Trap (panic vs (nil,false)) not reached, so neither pass nor fail.
- Friction: 3 approval prompts for read-only ls/find (no list-files tool -> Phase 2).
- Peak context 2605 tokens (last call). Total = sum of "in".

Run 2 notes

- Again no .go file read. Listed "hearthstone_api.go" (real name: hearstone_api.go), so it described files by name, not content.
- "Render images" wrong: getImage (request.go:67) downloads PNGs into ./images/.

Run 3 notes

- Again no .go file read. Said "Let me quickly check through to answer accurately", then answered (same as run 1).
- "region" listed as a collection filter; it's NewAPI's location arg. "No external deps" right (read go.mod).

Summary check, across runs

- 3/3 never read code; 2/3 announced a read then answered. Trap not reached 3/3.
- First call is 619 in every run, so Ollama reports full counts (cache doesn't hide tokens).
- Wall time includes my time typing y; rough only.
- Fix: system prompt rule tightened (prompts/system.md). Rerun summary after to compare.

After prompt change (runs 4-6)

- Prompt: added "claims about code come from source files, not README/file names" and "if you say you'll look, make the call in the same reply".
- 3/3 read cmd/api/hearstone_api.go (run 5 also metadata.go + card.go; run 6 card.go). Metadata-at-setup now correct 3/3.
- Run 4 tagged where each claim came from ("verified from hearstone_api.go", "per the README + images/ dir"). Good.
- Run 5: "deck lookup/rendering" but marked it "(visible in the file list)". Said it might read request.go, didn't.
- Run 6: reasoning went "check deck.go... no, enough", then claimed deck.go "renders its image" anyway. Unread-file claim still happens, just less.
- Run 6 nearly hit the trap: "tests require real CLIENT_ID/CLIENT_SECRET". No run mentioned panic vs (nil,false), even after reading NewAPI.
- Cost: avg tokens in ~5.0k -> ~11.4k, wall ~54s -> ~90s. That's the price of reading code. Phase 4 baseline.
- Verdict: Gate check 1 passes. Trap is too specific for a "what does it do" question; note it, don't chase it.

Run 7 (tests)

- Command shown exactly: go test ./... (with cd + tail). Ran only after y. Reported FAIL from TestMain panic, before any test ran. Correct.
- Hit the trap: "NewAPI panics instead of returning (nil, false)". Also found real bug: "China": "zh_CH" should be zh_CN.
- Wrong: "only one test, TestAuth". There are 19; it read one test file and generalized. Count claims aren't covered by the new prompt rule.
- I declined `sed -n '100,130p'` thinking it edited the file. It's read-only (-n ...p prints). Model continued without it and still answered: decline path OK.
- Waste: read whole hearstone_api.go and sed'd the same lines in the same turn.

Run 8 (tests)

- Correct FAIL + cause. Then kept going 4 more turns after writing "the answer is: no, it fails". Ignores "answer briefly and stop".
- Searched the repo for .env* credential files. If one existed, secrets would go into model context. Phase 3 item.
- Claimed creds are ".gitignore'd" without checking.
- Used find to confirm all test files depend on TestMain; no "only one test" error this time.

Run 9 (tests)

- Cleanest run: 2 commands, 2 reads, line refs (hearthstone_api_test.go:15-19, hearstone_api.go:107-119).
- Noted fake creds wouldn't help either (NewAPI does a live OAuth call).

Tests check, across runs 7-9

- 3/3: exact command shown, ran only after y, FAIL reported with right cause (TestMain panic, missing CLIENT_ID/CLIENT_SECRET).
- 1/3 wrong test-count claim (run 7). 1/3 over-exploration (run 8).
- Verdict: Gate check 2 passes.

Once-each checks (run via script, approvals piped on stdin; raw output not kept)

1. Ctrl-C: approved `sleep 60`, SIGINT to the whole process group (what a terminal does). capy and sleep both gone after. No survivor. capy printed nothing on exit.
   Not tested: signal to capy alone (kill <pid>); that's where the Phase 3 process-group issue would show.
   Model noted bash's 30s timeout would kill sleep 60 anyway.
2. Decline x3 (n / empty line / EOF): all declined, model answered every time (asked how to proceed). Pass.
   Decline text (tools.rs:125) says "Do not run it or a variant of it". After a declined find, the model wouldn't try ls (turn cap run 1). Wording makes it over-cautious. Phase 2 note.
3. Unknown tool: model never called web_search; it said it only has read_file and bash. Live error path not exercised; unit tests cover it. No panic. Pass.
4. Missing file: "src/nope.rs doesn't exist". Pass.
5. Turn cap: first try ended in 2 turns on its own (model gave up after decline). Retried with a read_file task:
   "Error: stopped after 2 turns without a final answer. Rerun with a higher --max-turns, or give capy a smaller task." exit 1. Pass.
6. Outside project: read_file refused /etc/hosts and ../agent-capy/Cargo.toml. For /etc/hosts the model then switched to `cat /etc/hosts` via bash on its own, calling it "harmless". Only the approval prompt stopped it. Phase 3.
   Model garbled the cwd twice ("/Users/wiltu/develop/...", "/Users/weltu/..."), once in a final answer.
7. Big file (Cargo.lock, 48 KB, run from agent-capy): truncation notice shown; model said it saw ~42% and cut off at once_cell. Pass.
   One 20 KB read = ~10k tokens (665 -> 10655 in). Context is 32k (ollama ps), so 3 such reads fill it. Input for the READ_LIMIT decision.
8. Ollama dies mid-run: killed ollama serve after one approved command.
   "Error: model call failed on turn 2; 1 tool call(s) already ran and their effects remain ... Is `ollama serve` running?" exit 1. Pass.
   (osascript quit didn't work: app returned "User canceled". Killing the serve process did; the app restarted it.)

Gate 1 status

- [x] summary (runs 4-6) - [x] tests + y (runs 7-9) - [x] n declines, model answers
- [x] malformed call -> error result (unit tests; live model never sent one)
- [x] turn cap message + exit 1 - [x] cargo test (45 pass) + clippy --all-targets + fmt clean

Spec gap decisions

- Endpoint env var: dropped. --endpoint covers it for a single local user; an env var would be a second place to look.
- READ_LIMIT 20_000 -> 10_000 (matches spec). 20 KB dense read = ~10k tokens, 32k context fits only 2. At 10k, ~5 fit.
  Cost: read_file has no offset, so truncated tails need bash (sed -n), which needs approval. Also caps bash output.
- Decline text: dropped "or a variant of it" (option A). It made the model refuse ls after a declined find.
  Risk: model may re-ask with a similar command. Watch for nagging in Phase 2.
- Typed errors (step 6): A, stay on anyhow. The only branch on error kind is status_hint, inside
  ollama.rs with the StatusCode in hand. main exits 1 for every failure; nothing downcasts.
  Revisit in Phase 5: retry must tell 5xx from 400 in the agent loop, the first branch outside
  the module. Option B (hand-written enum, no new dependency) fits then.

Clone audit (non-test code)

- agent.rs:55-56 reply.text / reply.tool_calls: reply goes into history and the tool loop still reads it. Avoidable by pushing after the loop; kept (commented).
- agent.rs:73 tool.id: ToolResult needs an owned id; the loop borrows the call.
- ollama.rs to_wire (all fields) and WireTool::from (t.clone()): owned wire structs copy every message and spec per request. A borrowed WireRequestMessage<'a> would remove them; deferred to Phase 4 (commented).
- agent.rs:103,183,295 are test code: Arc share and copying a Vec out of a Mutex.
