# Stickers

Images bigger than emoji, sent one per message.

| Request | |
|---|---|
| `GET /spaces/{space_id}/stickers` | members |
| `POST /spaces/{space_id}/stickers` `{name, description?, tags?, image}` | `manage_emojis`; PNG, GIF or WebP `data:` URI up to the emoji size limit; at most 60 per space |
| `PATCH /spaces/{space_id}/stickers/{sticker_id}` `{name?, description?, tags?}` | `manage_emojis` |
| `DELETE /spaces/{space_id}/stickers/{sticker_id}` | `manage_emojis` |
| `POST /channels/{channel_id}/stickers/{sticker_id}` `{reply_to?}` | sends it (space channels); a sticker of another space needs membership there and `use_external_stickers` or `use_external_emojis` in the channel |

Names are 2–30 characters, descriptions up to 100, `tags` (an emoji) up to 50.
Images are stored with the space's emoji under
`/cdn/emojis/{space_id}/sticker-<id>.<ext>`. Messages carry
`stickers: [{id, name, image, format}]` (a deleted sticker keeps its name).
Gateway: `sticker.create`, `sticker.update`, `sticker.delete`. Tables
`stickers`, `message_stickers` (migration 044, Postgres 029).
