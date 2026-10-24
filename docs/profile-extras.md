# Profile extras and notes

| Request | |
|---|---|
| `GET /users/@me/profile` | your pronouns and theme |
| `PATCH /users/@me/profile` `{pronouns?, theme_primary?, theme_accent?}` | pronouns up to 40 characters (`""`/null clears); colours `0xRRGGBB` (null clears) |
| `GET /users/{user_id}/profile` | someone's pronouns and theme, plus `note`: what *you* noted about them |
| `GET /users/@me/notes` | `{user_id: note}` for everyone you noted |
| `PUT /users/@me/notes/{user_id}` `{note}` | up to 256 characters; empty removes it; only you ever see it |

`user.profile_update` goes to the spaces you're in (and your sessions);
`user.note_update` only to your own sessions. Tables `user_profiles`,
`user_notes` (migration 045, Postgres 030).
