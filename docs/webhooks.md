# Incoming webhooks

A webhook is a secret URL that posts into one text (or announcement) channel.
Members with `manage_webhooks` create and manage them; anything holding the URL
can post. The payload is the one chat services' webhook integrations already
send, so monitoring and media tools (Uptime Kuma, Sonarr, Radarr, Jellyfin's
webhook plugin, CI notifications…) can be pointed at the URL unchanged.

## Managing

| Request | Notes |
|---|---|
| `POST /channels/{channel_id}/webhooks` `{name, avatar?}` | `avatar` is a `data:` URI; at most 15 per channel |
| `GET /channels/{channel_id}/webhooks` | with tokens |
| `GET /spaces/{space_id}/webhooks` | every webhook of the space |
| `GET /webhooks/{id}` | |
| `PATCH /webhooks/{id}` `{name?, avatar?, channel_id?}` | `avatar: ""` removes it; moves stay in the space |
| `DELETE /webhooks/{id}` | its past messages keep their author |

All need `manage_webhooks` in the webhook's channel. Changes are recorded in
the audit log (`webhook_create`, `webhook_update`, `webhook_delete`) and
announced to managers with a `webhooks.update` gateway event
(`{space_id, channel_id}`).

Each webhook posts as its own bot user (`bot: true`, not a member of the
space), named after the webhook, so its messages render like any author's.

## Posting

`POST /api/v1/webhooks/{id}/{token}` — no authentication, JSON body:

| Field | |
|---|---|
| `content` | up to 4000 characters |
| `username` | display name for this message only (shown via `webhook_author.name`) |
| `avatar_url` | accepted, not displayed (remote images are not loaded) |
| `embeds` | up to 10; parts that don't fit the embed model (an empty footer…) are dropped instead of failing |
| `allowed_mentions.parse` | `@everyone`/`@here` only notify when it contains `"everyone"` |
| `tts` | |

Answers `204 No Content`, or the created message with `?wait=true`.
A wrong id or token answers `404`. `GET` on the same URL returns the webhook
(without its token) so an integration can check it.

`PATCH`/`DELETE /api/v1/webhooks/{id}/{token}/messages/{message_id}` edit or
delete a message that webhook posted.

Messages posted by a webhook carry `webhook_id`, and `webhook_author: {name}`
when the call set a `username`.
