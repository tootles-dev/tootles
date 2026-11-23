---- MODULE BlueskyRefresh ----
(* Model of Bluesky session refresh in src/bluesky_auth.rs and the config
   writes in src/config.rs and src/main.rs.

   A "client" is one SessionClient state (bluesky_auth.rs `State` behind
   `Arc<Mutex<_>>`): one per Tootles process, or two in one process after an
   in-app account switch (app.rs `saved_provider`). A "task" is one call to
   SessionClient::request on a clone of that client.

   A token is [sid, gen]: `sid` is the server session that a login created,
   `gen` counts rotations. The access and refresh tokens rotate together, so
   one pair stands for both. *)
EXTENDS Naturals

CONSTANTS Clients, Tasks, MaxExpire, MaxLogin, AllowLogout, LostResponses,
          Faults   \* deliberate breaks of the code; empty when checking it

Broken(fault) == fault \in Faults

Procs == Clients \X Tasks
Sids == 0..MaxLogin
Token(s, g) == [sid |-> s, gen |-> g]
NoAccount == Token(99, 0)   \* config.toml has no such Bluesky account
Free == <<>>                \* a lock that nobody holds

VARIABLES
  srv,        \* server: current generation of each session
  expired,    \* server: the current access token of the session has expired
  expiries,   \* how many times an access token expired
  logins,     \* how many times `tootles login` ran again
  disk,       \* the account's token in config.toml
  mem,        \* State.session of each client
  saved,      \* State.saved_token of each client
  invalid,    \* State.invalid of each client
  stateLock,  \* holder of each client's tokio Mutex
  sessLock,   \* holder of the cross-process config::session_lock
  pc, tok, attempt, renewed, outcome,   \* per task
  envChanged, \* a login or logout changed the saved account
  spentSent,  \* a refresh token that the server had already rotated was sent
  chosen      \* the session that the user's last login or logout left saved

server == <<srv, expired, expiries, logins>>
client == <<mem, saved, invalid>>
locks == <<stateLock, sessLock>>
hist == <<envChanged, spentSent, chosen>>
vars == <<server, disk, client, locks, pc, tok, attempt, renewed, outcome, hist>>

Init ==
  /\ srv = [s \in Sids |-> 0]
  /\ expired = [s \in Sids |-> FALSE]
  /\ expiries = 0
  /\ logins = 0
  /\ disk = Token(0, 0)
  /\ mem = [c \in Clients |-> Token(0, 0)]
  /\ saved = [c \in Clients |-> Token(0, 0)]
  /\ invalid = [c \in Clients |-> FALSE]
  /\ stateLock = [c \in Clients |-> Free]
  /\ sessLock = Free
  /\ pc = [p \in Procs |-> "idle"]
  /\ tok = [p \in Procs |-> Token(0, 0)]
  /\ attempt = [p \in Procs |-> 0]
  /\ renewed = [p \in Procs |-> Token(0, 0)]
  /\ outcome = [p \in Procs |-> "none"]
  /\ envChanged = FALSE
  /\ spentSent = FALSE
  /\ chosen = 0

Goto(p, label) ==
  pc' = [pc EXCEPT ![p] = label] /\ UNCHANGED <<attempt, outcome>>
Retry(p) ==   \* request(): `continue` into the second attempt
  /\ pc' = [pc EXCEPT ![p] = "idle"]
  /\ attempt' = [attempt EXCEPT ![p] = 1]
  /\ UNCHANGED outcome
Finish(p, result) ==
  /\ pc' = [pc EXCEPT ![p] = "done"]
  /\ outcome' = [outcome EXCEPT ![p] = result]
  /\ UNCHANGED attempt
Release(c) ==   \* refresh() returns: the state guard and the session guard drop
  stateLock' = [stateLock EXCEPT ![c] = Free] /\ sessLock' = Free
MarkInvalid(c) == invalid' = [invalid EXCEPT ![c] = TRUE]

(* request(): lock State, check `invalid`, copy the access token, unlock.
   SessionClient::request in bluesky_auth.rs, one critical section with no await
   inside. *)
ReadToken(p) ==
  LET c == p[1] IN
  /\ pc[p] = "idle"
  /\ stateLock[c] = Free
  /\ IF invalid[c]
       THEN Finish(p, "unavailable") /\ UNCHANGED tok
       ELSE Goto(p, "send") /\ tok' = [tok EXCEPT ![p] = mem[c]]
  /\ UNCHANGED <<server, disk, client, locks, renewed, hist>>

Accepted(t) == t.gen = srv[t.sid] /\ ~expired[t.sid]

(* request(): send, then succeed, start a refresh (attempt 0), or give up with
   the HTTP 401 error (attempt 1). *)
SendRequest(p) ==
  /\ pc[p] = "send"
  /\ IF Accepted(tok[p]) THEN Finish(p, "ok")
     ELSE IF attempt[p] = 0 THEN Goto(p, "want_refresh")
     ELSE Finish(p, "rejected")
  /\ UNCHANGED <<server, disk, client, locks, tok, renewed, hist>>

(* refresh(): lock State and keep it. Return early when the client is invalid
   or when another task already replaced the rejected token. *)
StartRefresh(p) ==
  LET c == p[1] IN
  /\ pc[p] = "want_refresh"
  /\ stateLock[c] = Free
  /\ IF invalid[c] THEN Finish(p, "unavailable") /\ UNCHANGED stateLock
     ELSE IF mem[c] # tok[p] /\ ~Broken("no_recheck") THEN Retry(p) /\ UNCHANGED stateLock
     ELSE Goto(p, "want_session_lock") /\ stateLock' = [stateLock EXCEPT ![c] = p]
  /\ UNCHANGED <<server, disk, client, sessLock, tok, renewed, hist>>

(* refresh(): config::session_lock, a blocking flock. *)
TakeSessionLock(p) ==
  /\ pc[p] = "want_session_lock"
  /\ (sessLock = Free \/ Broken("no_session_lock"))
  /\ sessLock' = p
  /\ Goto(p, "load")
  /\ UNCHANGED <<server, disk, client, stateLock, tok, renewed, hist>>

(* refresh(): Config::load, then adopt a token that another client or a new
   login saved. *)
LoadConfig(p) ==
  LET c == p[1] IN
  /\ pc[p] = "load"
  /\ IF disk = NoAccount
       THEN MarkInvalid(c) /\ UNCHANGED <<mem, saved>> /\ Release(c)
            /\ Finish(p, "sign_in_again")
     ELSE IF disk # saved[c] /\ ~Broken("no_adopt")
       THEN /\ mem' = [mem EXCEPT ![c] = disk]
            /\ saved' = [saved EXCEPT ![c] = disk]
            /\ UNCHANGED invalid
            /\ IF disk # tok[p] THEN Release(c) /\ Retry(p)
               ELSE UNCHANGED locks /\ Goto(p, "http")
     ELSE UNCHANGED <<client, locks>> /\ Goto(p, "http")
  /\ UNCHANGED <<server, disk, tok, renewed, hist>>

(* refresh(): POST refreshSession with State.session.refresh_jwt. The server
   rotates a current token and rejects a spent one. *)
SendRefresh(p) ==
  LET c == p[1]
      t == mem[c] IN
  /\ pc[p] = "http"
  /\ IF t.gen = srv[t.sid]
       THEN /\ srv' = [srv EXCEPT ![t.sid] = @ + 1]
            /\ expired' = [expired EXCEPT ![t.sid] = FALSE]
            /\ renewed' = [renewed EXCEPT ![p] = Token(t.sid, t.gen + 1)]
            /\ Goto(p, "persist")
            /\ mem' = IF Broken("use_before_save")
                        THEN [mem EXCEPT ![c] = Token(t.sid, t.gen + 1)] ELSE mem
            /\ UNCHANGED <<expiries, logins, saved, invalid, locks, hist>>
       ELSE /\ spentSent' = TRUE
            /\ MarkInvalid(c) /\ Release(c)
            /\ Finish(p, "sign_in_again")
            /\ UNCHANGED <<server, mem, saved, renewed, envChanged, chosen>>
  /\ UNCHANGED <<disk, tok>>

(* refresh(): the POST never reaches the server. `map_err(transport_error)?`
   returns without setting `invalid`. *)
RefreshNeverSent(p) ==
  /\ pc[p] = "http"
  /\ Release(p[1])
  /\ Finish(p, "network_error")
  /\ UNCHANGED <<server, disk, client, tok, renewed, hist>>

(* The same line, but the server rotated the token and the reply was lost.
   Off by default: see LostResponses in the config. *)
RefreshReplyLost(p) ==
  LET t == mem[p[1]] IN
  /\ LostResponses
  /\ pc[p] = "http"
  /\ t.gen = srv[t.sid]
  /\ srv' = [srv EXCEPT ![t.sid] = @ + 1]
  /\ expired' = [expired EXCEPT ![t.sid] = FALSE]
  /\ Release(p[1])
  /\ Finish(p, "network_error")
  /\ UNCHANGED <<expiries, logins, disk, client, tok, renewed, hist>>

(* refresh(): config::update writes the renewed token only over the token
   that this client last saw; then State takes it. *)
SaveRenewed(p) ==
  LET c == p[1] IN
  /\ pc[p] = "persist"
  /\ IF disk = saved[c] \/ Broken("blind_save")
       THEN /\ disk' = renewed[p]
            /\ mem' = IF Broken("forget_new_token")
                        THEN mem ELSE [mem EXCEPT ![c] = renewed[p]]
            /\ saved' = [saved EXCEPT ![c] = renewed[p]]
            /\ UNCHANGED invalid
            /\ Release(c) /\ Retry(p)
       ELSE /\ MarkInvalid(c) /\ UNCHANGED <<disk, mem, saved>>
            /\ Release(c) /\ Finish(p, "sign_in_again")
  /\ UNCHANGED <<server, tok, renewed, hist>>

AccessTokenExpires ==
  /\ expiries < MaxExpire
  /\ \E s \in 0..logins :
       /\ ~expired[s]
       /\ expired' = [expired EXCEPT ![s] = TRUE]
  /\ expiries' = expiries + 1
  /\ UNCHANGED <<srv, logins, disk, client, locks, pc, tok, attempt, renewed, outcome, hist>>

(* `tootles login` for the same account: the Commands::Login arm in main.rs replaces the
   saved account with a new server session. *)
LoginAgain ==
  /\ logins < MaxLogin
  /\ logins' = logins + 1
  /\ disk' = Token(logins + 1, 0)
  /\ envChanged' = TRUE
  /\ chosen' = logins + 1
  /\ UNCHANGED <<srv, expired, expiries, client, locks, pc, tok, attempt, renewed, outcome, spentSent>>

(* `tootles logout`: the Commands::Logout arm in main.rs removes the saved account. *)
Logout ==
  /\ AllowLogout
  /\ disk # NoAccount
  /\ disk' = NoAccount
  /\ envChanged' = TRUE
  /\ chosen' = NoAccount.sid
  /\ UNCHANGED <<server, client, locks, pc, tok, attempt, renewed, outcome, spentSent>>

AllDone == (\A p \in Procs : pc[p] = "done") /\ UNCHANGED vars

Next ==
  \/ \E p \in Procs : ReadToken(p)
  \/ \E p \in Procs : SendRequest(p)
  \/ \E p \in Procs : StartRefresh(p)
  \/ \E p \in Procs : TakeSessionLock(p)
  \/ \E p \in Procs : LoadConfig(p)
  \/ \E p \in Procs : SendRefresh(p)
  \/ \E p \in Procs : RefreshNeverSent(p)
  \/ \E p \in Procs : RefreshReplyLost(p)
  \/ \E p \in Procs : SaveRenewed(p)
  \/ AccessTokenExpires
  \/ LoginAgain
  \/ Logout
  \/ AllDone

Spec == Init /\ [][Next]_vars

----
(* Each refresh token is sent to the server at most once. *)
NoSpentRefreshTokenSent == ~spentSent

(* The saved session works at the next launch: config.toml holds the token
   the server treats as current, except while one refresh is between the
   server's reply and its save. *)
SavedSessionIsCurrent ==
  (disk # NoAccount /\ disk.gen # srv[disk.sid]) =>
    \E p \in Procs : pc[p] = "persist" /\ renewed[p].sid = disk.sid

(* No client holds or sends a token newer than the saved one. *)
NeverUsesUnsavedCredentials ==
  \A c \in Clients :
    (disk # NoAccount /\ mem[c].sid = disk.sid) => mem[c].gen <= disk.gen

(* A client asks the user to sign in again only after a login or logout. *)
NoFalseSignOut == \A c \in Clients : invalid[c] => envChanged

(* A request ends with "session was rejected" only when a token expired
   twice during that request. *)
NoFalseRejection == \A p \in Procs : outcome[p] = "rejected" => expiries >= 2

(* A refresh never brings back an account after logout, and never replaces
   the session of a newer login. *)
RefreshKeepsLoginAndLogout == disk.sid = chosen

(* The server rotates a session only after an access token expired. *)
RefreshOnlyAfterExpiry ==
  srv[0] + (IF MaxLogin >= 1 THEN srv[1] ELSE 0) <= expiries

StateMatchesSavedToken == \A c \in Clients : mem[c] = saved[c]
====
