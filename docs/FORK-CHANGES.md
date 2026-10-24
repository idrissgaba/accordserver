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
| Incoming webhooks (chat-service compatible payload) | `feature/webhooks` | Deployed | `migrations/041_webhooks.sql`, `migrations/postgres/026_webhooks.sql`, `src/models/webhook.rs`, `src/db/webhooks.rs`, `src/routes/webhooks.rs`, `tests/webhooks.rs`, `docs/webhooks.md` |
| Slash commands, interactions, message components | `feature/interactions` | Deployed | `migrations/042_interactions.sql`, `migrations/postgres/027_interactions.sql`, `src/models/app_command.rs`, `src/db/app_commands.rs`, `src/db/interactions.rs`, `src/routes/interactions.rs`, `src/routes/applications.rs`, `tests/interactions.rs`, `docs/interactions.md` |
| Scheduled events (start/end on their own, interested users) | `feature/scheduled-events` | Deployed | `migrations/043_scheduled_events.sql`, `migrations/postgres/028_scheduled_events.sql`, `src/models/scheduled_event.rs`, `src/db/scheduled_events.rs`, `src/routes/scheduled_events.rs`, `src/main.rs`, `tests/scheduled_events.rs`, `docs/scheduled-events.md` |
| Stickers | `feature/stickers` | Deployed | `migrations/044_stickers.sql`, `migrations/postgres/029_stickers.sql`, `src/db/stickers.rs`, `src/routes/stickers.rs`, `src/routes/messages.rs`, `tests/stickers.rs`, `docs/stickers.md` |
| Profile extras: pronouns, theme colours, private notes | `feature/profile-extras` | Deployed | `migrations/045_profile_extras.sql`, `migrations/postgres/030_profile_extras.sql`, `src/db/profiles.rs`, `src/routes/profiles.rs`, `tests/profiles.rs`, `docs/profile-extras.md` |
| Welcome screen | `feature/welcome-screen` | Deployed | `migrations/046_welcome_screens.sql`, `migrations/postgres/031_welcome_screens.sql`, `src/routes/welcome_screen.rs`, `tests/welcome_screen.rs`, `docs/welcome-screen.md` |
| Stage channels | `feature/stage-channels` (on `feature/voice-moderation`) | Deployed | `src/voice/stage.rs`, `src/voice/moderation.rs`, `src/models/voice.rs`, `src/security.rs`, `tests/stage_channels.rs`, `docs/stage-channels.md` |
| Super reactions | `feature/super-reactions` | Deployed | `migrations/047_super_reactions.sql`, `migrations/postgres/032_super_reactions.sql`, `src/db/super_reactions.rs`, `src/routes/super_reactions.rs`, `src/routes/messages.rs`, `tests/super_reactions.rs`, `docs/super-reactions.md` |

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

### Webhooks

See [webhooks.md](webhooks.md). Each webhook posts as its own bot user, so
official clients render its messages normally (they ignore `webhook_author`).

### Slash commands and components

See [interactions.md](interactions.md). Replaces upstream's stub
`/applications/{id}/commands` and `/interactions/{id}/{token}/callback`
routes; `GET /applications/@me` also answers a bot token with the bot's own
application. Official clients show bot answers as plain messages (no command
picker, no buttons).

### Scheduled events, stickers, profile extras, welcome screen

See [scheduled-events.md](scheduled-events.md), [stickers.md](stickers.md),
[profile-extras.md](profile-extras.md) and [welcome-screen.md](welcome-screen.md).
All are new routes and tables; messages gain a `stickers` field. Official
clients ignore them. Scheduled events are started and ended by a 15-second
ticker spawned in `main.rs`.

### Stage channels

See [stage-channels.md](stage-channels.md). A `stage` channel joins like a
voice channel; the audience is suppressed in LiveKit until a moderator
(`mute_members`) brings them up. Voice states gain
`request_to_speak_timestamp`, omitted when unset. Official clients don't know
the `stage` type; the server treats their joins like any other.

### Super reactions

See [super-reactions.md](super-reactions.md): own table, own routes and
`super_reaction.*` events; plain reaction counts are unchanged, so official
clients don't see them.

Migration numbers in this fork: 040 polls, 041 webhooks, 042 interactions,
043 scheduled events, 044 stickers, 045 profile extras, 046 welcome screens,
047 super reactions (Postgres 025–032). If upstream ships migrations with these numbers, renumber
the fork's on the next update and note it here.

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
