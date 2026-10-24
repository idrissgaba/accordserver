# Super reactions

A showier reaction, free for everyone here. Stored in `super_reactions`
(migration 047, Postgres 032) apart from plain reactions, so clients that
don't know them see unchanged reaction counts.

| Route | |
| --- | --- |
| `PUT /channels/{channel_id}/messages/{message_id}/super-reactions/{emoji}` | Add yours (needs `add_reactions`; refused while timed out) |
| `DELETE …/super-reactions/{emoji}` | Remove yours |

Messages carry `super_reactions: [{ "emoji", "count", "me" }]` (first added
first; absent when none). Changes are announced as `super_reaction.add` and
`super_reaction.remove` with `channel_id`, `message_id`, `user_id`, `emoji`,
under the `message_reactions` intent. Super reactions stay local: they are
refused on spaces homed on another server.
