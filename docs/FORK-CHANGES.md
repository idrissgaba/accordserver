# Fork changes (idrissgaba/accordserver)

Personal fork of [DaccordProject/accordserver](https://github.com/DaccordProject/accordserver)
(GPL-3.0, credit to the Daccord project), deployed on a private, invite-only
community server alongside the client fork
[idrissgaba/daccord](https://github.com/idrissgaba/daccord).

## Branches

| Branch | Purpose |
|---|---|
| `master` | Mirror of upstream `master` — never commit here |
| `feature/<name>` | One feature per branch, based on `master` |
| `idris/main` | Integration branch that is deployed: `master` + every feature branch merged |

## Features

| Feature | Branch | State | Files |
|---|---|---|---|
| Registration with an invite under `invite_only` | `feature/invite-only-registration` | Deployed | `src/routes/auth.rs`, `src/routes/invites.rs`, `src/db/invites.rs`, `tests/e2e.rs` |
| Mark unread (manual ack) | `feature/mark-unread` | Deployed | `src/routes/read_states.rs`, `src/db/read_states.rs`, `tests/e2e.rs` |
| Polls | `feature/polls` | Deployed | `migrations/040_polls.sql`, `migrations/postgres/025_polls.sql`, `src/models/poll.rs`, `src/db/polls.rs`, `src/routes/polls.rs`, `src/routes/messages.rs`, `tests/polls.rs` |
| Voice moderation enforced in LiveKit; move, disconnect, user limits | `feature/voice-moderation` | Deployed | `src/voice/moderation.rs`, `src/voice/livekit.rs`, `src/routes/members.rs`, `src/routes/voice.rs`, `src/gateway/mod.rs`, `src/security.rs`, `tests/voice_moderation.rs`, `docs/voice-moderation.md` |

### Registration with an invite (`invite_only`)

Upstream's `invite_only` policy refuses every registration and admins cannot
create accounts. Here `POST /api/v1/auth/register` accepts an optional
`invite_code`; under `invite_only` it must be redeemable (exists, unexpired,
uses left). The invite is redeemed before the default-space auto-join (so an
invite to that space still counts a use) and the account is removed again if it
loses the race for the last use. Compatible with official clients: they don't
send the field, which keeps upstream's behaviour.

### Mark unread

`POST /channels/{id}/ack` accepts `"manual": true`, which sets the read
position exactly (backwards included) instead of only moving it forward, and
the `read_state.update` broadcast carries `"manual"`. Without the flag the
behaviour is upstream's.

### Polls

`POST /channels/{id}/polls` creates a message with a poll (question, up to 10
answers, 1 hour to 32 days, single or multiple choice; space channels only).
`PUT`/`DELETE /channels/{id}/polls/{message_id}/answers/{answer_id}/@me` votes
and withdraws; `POST .../expire` ends it early (author or `manage_messages`).
Messages carry `poll` (counts, `me` for the caller, `closed`); votes broadcast a
`message.update`. New tables: `polls`, `poll_answers`, `poll_votes`
(migration 040; Postgres 025). Official clients ignore the `poll` field.

### Voice moderation

See [voice-moderation.md](voice-moderation.md): server mute/deafen and the
`speak`/`stream` rights are enforced in LiveKit (token grants, then
`UpdateParticipant` mid-call); `PATCH` member `channel_id` moves (or, `null`,
disconnects) a member; `user_limit` is enforced on join. Official clients keep
working: they see the real `mute`/`deaf`/`suppress` flags, and a moved official
client is evicted and rejoins its old channel (it doesn't know the `moved`
event). `livekit-protocol` (already a dependency of `livekit-api`) is named in
`Cargo.toml` for `ParticipantPermission`.

## Updating from upstream

```bash
git fetch upstream
git checkout master && git merge --ff-only upstream/master && git push origin master
for b in $(git branch --format='%(refname:short)' --list 'feature/*'); do
  git checkout "$b" && git rebase master   # or merge; fix conflicts, run cargo test
done
git checkout idris/main && git reset --hard master
for b in $(git branch --format='%(refname:short)' --list 'feature/*'); do git merge --no-ff "$b"; done
cargo test --no-fail-fast    # config::tests::test_data_dir_redirects_paths fails on Windows only
git push --force-with-lease origin idris/main 'feature/*' master
```

Then redeploy (see the deployment README on the server, `/srv/data/stack/daccord/README.md`).
