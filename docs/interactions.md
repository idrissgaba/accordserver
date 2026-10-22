# Slash commands and message components

Applications (bots) register slash commands; members use them from the
message box, or click buttons and pick from select menus on a bot's messages.
The server checks every invocation, then hands it to the bot over the gateway
with a token the bot answers with.

## Registering commands (the application's bot or owner)

| Request | |
|---|---|
| `GET /applications/{app_id}/commands` | the application's global commands |
| `PUT /applications/{app_id}/commands` `[definition…]` | make the list exactly this (do it at startup) |
| `POST /applications/{app_id}/commands` `definition` | create, or replace the one with that name |
| `DELETE /applications/{app_id}/commands/{command_id}` | |

A definition is `{name, description, options: [{name, description, type,
required, choices?, min_value?, max_value?, min_length?, max_length?}]}`.
Names are 1–32 lowercase letters, digits, `-` or `_`; descriptions 1–100
characters; required options come first. `type` is `string`, `integer`,
`number`, `boolean`, `user`, `channel` or `role` (the numeric codes 3, 4, 10, 5,
6, 7, 8 that bot libraries send are accepted too). At most 100 commands per
application, 25 options per command, 25 choices per option.
Members of every space the bot is in get an `application_commands.update`
event when the list changes.

## Using them (members)

`GET /spaces/{space_id}/commands` lists the commands of the bots that are
members of the space, each with `application: {id, name, bot_user_id}`.

`POST /interactions`:

- a command: `{type: "application_command", channel_id, command_id,
  data: {options: [{name, value}]}}` — options are checked against the
  definition (types, bounds, choices; users, channels and roles must be in the
  space). Numbers typed as text are accepted.
- a component: `{type: "message_component", channel_id, message_id,
  data: {custom_id, values?}}` — the component must exist on that bot message
  and not be disabled; select values must be among its options.

The caller needs `send_messages` in the channel (space channels only). The
answer is `{id}`; the bot's user receives an `interaction.create` event:
`{id, application_id, type, token, space_id, channel_id, user, data,
message?}` (`data` = `{id, name, options: [{name, type, value}]}` or
`{custom_id, component_type, values?}`; `message` = the clicked message).

## Answering (the application, with the token, for 15 minutes)

`POST /interactions/{id}/{token}/callback` `{type, data?}` — once:

| type | |
|---|---|
| `message` (4) | posts `data` as the bot's message, marked as answering the command (`interaction: {id, type, name, user_id}`) |
| `deferred_message` (5) | posts a "thinking…" placeholder (flag 128) to fill in later |
| `deferred_update` (6) | component only: acknowledge, change nothing yet |
| `update_message` (7) | component only: edits the clicked message |
| `pong` (1) | no-op |

`data`: `{content?, embeds?, components?, flags?, tts?}`. Flag 64 makes the
answer ephemeral: it is not stored, and only the member's sessions get it (as
`message.create` with flag 64). The member gets `interaction.success {id}`
once the bot has answered.

Then:

- `PATCH /interactions/{id}/{token}/original` edits the answer (filling a
  placeholder clears its flag and does not mark it edited), or — for a
  component interaction answered with 6/7 — the clicked message;
- `DELETE /interactions/{id}/{token}/original`;
- `POST /interactions/{id}/{token}/followup` posts another message
  (ephemeral allowed).

## Components

Up to 5 rows (`{type: "action_row" | 1, components: […]}`), each with up to 5
buttons or one select menu:

- button `{type: "button" | 2, style, label?, emoji?, custom_id, disabled?}`;
  `style` is `primary`, `secondary`, `success`, `danger` (1–4) or `link` (5,
  with `url` instead of `custom_id`);
- select `{type: "select" | 3, custom_id, options: [{label, value,
  description?, emoji?, default?}], placeholder?, min_values?, max_values?}`.

Custom ids are unique within a message. Besides interaction answers, a bot
sets the components of its own message with
`PUT /channels/{channel_id}/messages/{message_id}/components {components}`
(`[]` or `null` removes them). Messages carry `components` and, when they
answer a command, `interaction`. Stored in `message_components` and
`message_interactions`; interactions in `interactions` (migration 042,
Postgres 027).
