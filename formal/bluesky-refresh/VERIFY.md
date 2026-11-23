# Verify Bluesky session refresh

A Bluesky refresh token works once. Tootles can run several requests and several processes on one account, and each can meet an expired access token at the same moment. If two of them spend the same refresh token, or one saves over the other, the user is signed out and must create a new app password. The tests in `tests/bluesky_login.rs` cover a few timed orderings. The TLA+ model in this folder covers every ordering within its bounds.

## Does your change touch it?

| Code | What the model takes from it | Model action |
|------|------------------------------|--------------|
| `src/bluesky_auth.rs` `SessionClient::request` | reads the token under the State lock, sends, retries once after a refresh | `ReadToken`, `SendRequest` |
| `src/bluesky_auth.rs` `SessionClient::refresh` | the order: State lock, early return, session lock, load config, adopt a newer token, POST refreshSession, save, update State. The session lock is held until `refresh` returns. | `StartRefresh`, `TakeSessionLock`, `LoadConfig`, `SendRefresh`, `RefreshNeverSent`, `SaveRenewed` |
| `src/config.rs` `update`, `session_lock`, `private_lock` | `update` reads and writes config.toml as one step under the config lock; `session_lock` is exclusive across processes, because `private_lock` waits in `file.lock()` | `SaveRenewed`, `TakeSessionLock` |
| `src/config.rs` `write_private` | the new config.toml appears in one step, at the rename. `Config::load` takes no lock, so a reader sees the old file until then. | `SaveRenewed`, `LoadConfig` |
| `src/main.rs` `Commands::Login`, `Commands::Logout` | login replaces the saved account with a new session, logout removes it, both through `config::update` | `LoginAgain`, `Logout` |
| `src/app.rs` `saved_provider` | an account switch builds a second `SessionClient` in the same process from the saved token | a second member of `Clients` |

Any change to the order of steps, to what a lock covers or when it is released, to when `State.invalid` is set, or to what `config::update` compares in these functions needs the check. A change to error wording, HTTP headers, or response decoding does not. A new writer of account tokens in config.toml, or a new caller that builds a `SessionClient`, is covered code even in a new file.

The lines below are the steps that the model depends on, in the order they appear in each file. The hash after each path covers the text from the first cited line to the last.

```
covered:
  - src/bluesky_auth.rs 84f20b35540f
    > async fn request(
    > let state = self.state.lock().await;
    > request.send().await
    > client.refresh(&token).await
    > async fn refresh(&self, rejected: &str)
    > let mut state = self.state.lock().await;
    > config::session_lock(
    > Config::load(self.root.as_deref())
    > if account.token != state.saved_token
    > com.atproto.server.refreshSession
    > config::update(self.root.as_deref()
    > a.token == state.saved_token
    > state.saved_token = token;
  - src/config.rs e6eebabc2c26
    > pub fn update<T>(
    > private_lock(&root, ".config.lock")
    > Config::load(Some(&root))
    > write_private(&root, "config.toml"
    > pub(crate) fn session_lock(
    > .session-{:x}.lock
    > fn private_lock(
    > file.lock()
  - src/config.rs 594092228a75
    > fn write_private(
    > fs::rename(&temporary, &target)
  - src/main.rs 9eb2dfc2aee7
    > config::update(cli.config_dir.as_deref(), |latest| {
    > latest.accounts.retain(|a| a.id != account.id);
    > latest.accounts.push(account);
  - src/main.rs 9754ac5b5902
    > Commands::Logout { account } => {
    > latest.accounts.retain(|a| a.id != id);
    > latest.active = latest.accounts.first()
  - src/app.rs 8d7f4535f0fe
    > fn saved_provider(
    > Bluesky::from_saved(account, root)
```

`drift.sh` reports a stretch whose text changed since the last sync, and names a cited line that is missing or moved. It is in the `maintain-formal-verification` skill of the `formal-proofs` plugin. Run it from the repo root: `sh <that skill's folder>/drift.sh`.

## The rules

| Rule | What it promises | Harm if it breaks |
|------|------------------|-------------------|
| `NoSpentRefreshTokenSent` | No refresh token is sent to the server twice. | The server refuses the refresh, and that client tells the user to sign in again. |
| `NoFalseSignOut` | A client shows "sign in again" only after a real login or logout. | The user is signed out for no reason. |
| `SavedSessionIsCurrent` | config.toml holds the token the server accepts, except inside one refresh between the server's reply and the save. | The next launch starts with a dead session. |
| `NeverUsesUnsavedCredentials` | No request uses a token newer than the saved one. | A crash loses the only copy of the working token. |
| `RefreshKeepsLoginAndLogout` | A refresh never brings back a logged-out account or replaces a newer login. | Credentials the user removed reappear on disk. |
| `NoFalseRejection` | A request fails with "session was rejected" only if a token expired twice during it. | A working session shows a sign-in error. |
| `RefreshOnlyAfterExpiry` | The server rotates a session only after an access token expired. | Needless rotations widen every race above. |
| `StateMatchesSavedToken` | `State.session` and `State.saved_token` always describe the same token. | The save compares against the wrong token and fails or overwrites. |
| no deadlock | TLC's built-in check, not listed in the config: no reachable state is stuck with every request and lock waiting. It does not prove that each request finishes. | The interface hangs on a Bluesky call. |

## Run the check

Check `formal/bluesky-refresh/BlueskyRefresh.tla` with the `tla-check` skill. The config `BlueskyRefresh.cfg` sits next to it and is picked up by default. A run takes about 3 seconds and finds 722,238 distinct states at the last sync; the count changes with any edit to the model. The skill comes from the `formal-proofs` plugin, version 0.2.0 or later, https://github.com/jaschadub/skill-formal-proofs. If the plugin is not installed, clone that repo: `skills/tla-check/SKILL.md` and `skills/maintain-formal-verification/drift.sh` are in it.

Every rule must hold with `Faults = {}` and `LostResponses = FALSE`. Every action must also run: list the count for each action as `tla-check` describes. Only `RefreshReplyLost` may show `0:0`, because `LostResponses` is off.

A pass on a model that no one updated says nothing about a change to the code. With a change to the covered code in the tree, the unchanged model still passes with the same count. Update the model first.

## When a change touches the covered code

These steps are for your own change, and for a change that you review before it lands. For a change that landed without them, follow the `maintain-formal-verification` skill.

1. If the change alters nothing that the model describes, such as a rename of a local variable, confirm that by reading it against the table and the model, and go to step 6.
2. Change the model so that it describes the new code, step for step. Write this from the code before you run anything. A step in the model ends where another task or process can first see a difference: at an `await` that lets another request run, at a lock release, or at a write to config.toml. A lock release that the code now does at a new place is a step of its own. When a step moves or changes, also update its cited line above and the table. `Release(c)` in the model frees both the State lock and the session lock: if a lock moves, check every action that uses it, so that no action frees a lock it does not hold. When you add a step, a rule that names steps, such as `SavedSessionIsCurrent`, must name the new step too. That does not weaken the rule, as long as its sentence in the table above stays true.
3. Run the check.
4. If a rule breaks, replay the path against the real code, one step for each action. If a step cannot happen in the code, the model is wrong: fix the model. If every step can happen, it is a bug in the change: report it, do not weaken the rule, and do not sync. Keep the updated model and record with the change, as a patch or on its branch, so that a fix can be checked against them. TLC names only the first rule that breaks; to list the others, check one rule at a time as `tla-check` describes. A read-through of the code is the least that counts as a replay. A test is better evidence, but only one that can reach the path: a path with two clients needs a multi-thread runtime or two processes, and `parallel_clients_refresh_once_and_persist_before_retried_requests` in `tests/bluesky_login.rs` runs on one thread.
5. If every rule holds, confirm that the check can still fail: run each fault below and see that its rules break.
6. Only then record the new state of the code: from the repo root, run `drift.sh --sync formal/bluesky-refresh/VERIFY.md`, check the line numbers that it prints, and commit the record with the change. One stretch can hold several changes; sync only when each of them is settled.

## Faults that must break a rule

`Faults` in the config switches on one deliberate break of the code. Set it in a copy of the config in a scratch folder outside the repo, such as `Faults = {"no_session_lock"}`, and check the spec with that copy. To see the second rule of a fault, keep only that rule in the copy. Each fault must break the rules listed. If one stops breaking them, the model has lost the ability to fail there.

| Fault | The break in the code | Rules that must break |
|-------|-----------------------|-----------------------|
| `no_recheck` | `refresh` no longer returns early when the token was already replaced | `RefreshOnlyAfterExpiry`, `NoFalseRejection` |
| `no_session_lock` | `refresh` does not wait for `config::session_lock` | `NoSpentRefreshTokenSent`, `NoFalseSignOut` |
| `no_adopt` | `refresh` ignores a newer token in config.toml | `NoSpentRefreshTokenSent`, `NoFalseSignOut` |
| `blind_save` | `config::update` writes without comparing `saved_token` | `RefreshKeepsLoginAndLogout` |
| `use_before_save` | `State.session` takes the renewed token before the save | `NeverUsesUnsavedCredentials`, `StateMatchesSavedToken` |
| `forget_new_token` | the save succeeds but `State.session` keeps the old token | `StateMatchesSavedToken`, `NoFalseRejection` |

## Known weaknesses of the design

The model includes these cases and shows that they break a rule. No order of steps in the client prevents them.

| Case | How to see it | What breaks | Harm |
|------|---------------|-------------|------|
| The server rotates the token and its reply never arrives. | Set `LostResponses = TRUE` in a copy of the config in a scratch folder. | `SavedSessionIsCurrent` | The only copy of the new refresh token is lost. The user must sign in again with a new app password. A server that accepts the previous refresh token for a grace period would prevent it. |

## What the check does not cover

- **Bounds:** two clients, two requests each, two expiries, one extra login, any number of logouts. A bug that needs three clients or three expiries is out of reach.
- **A crash or kill** between the server's reply and the save. It is not modeled; it ends the same way as the lost reply above.
- **A bad reply to a refresh:** `refresh` also sets `State.invalid` when the server returns invalid session data or a changed identity or PDS. The model has only a good reply and a refusal.
- **Disk failure** in `config::update`, and anything about file permissions or symlink checks.
- **The link to the code is by hand.** The model does not read Rust. It is true to the code only as long as this record is followed.
- **Server behavior:** the model's server refuses a refresh token that was already used, and rejects every access token from an older rotation. A real server may accept an older access token until it expires, which only removes refreshes. What a real server does to the rest of the session after a reused refresh token is not modeled.
- Mastodon accounts, which have no refresh.
