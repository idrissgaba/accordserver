# Scheduled events

Members with `manage_events` plan events in a voice channel of the space, or
somewhere else; any member marks themselves interested.

| Request | |
|---|---|
| `GET /spaces/{space_id}/events` | coming up or under way, soonest first, each with `user_count` and `interested` (the caller) |
| `POST /spaces/{space_id}/events` | `{name, description?, entity_type: "voice"\|"external", channel_id?, location?, start_time, end_time?, image?}` |
| `GET /spaces/{space_id}/events/{event_id}` | |
| `PATCH /spaces/{space_id}/events/{event_id}` | edit, or `status: "active"` (start now), `"completed"` (end), `"canceled"`; `image: ""` removes the cover |
| `DELETE /spaces/{space_id}/events/{event_id}` | |
| `GET /spaces/{space_id}/events/{event_id}/users` | ids of the interested members |
| `PUT` / `DELETE /spaces/{space_id}/events/{event_id}/users/@me` | interested / not any more |

Rules: a 1–100 character name, a description of up to 1000; a voice event takes
place in a voice channel of the space; an external one needs a location (up to
100 characters) and an end time; the end is after the start; the start is not
in the past (5 minutes of slack). An event under way can't be rescheduled.
Times are RFC 3339 and stored in UTC. The cover image (`data:` URI) is stored
under `/cdn/banners/event-<id>.*`. The creator starts out interested.

Lifecycle: `scheduled` → `active` → `completed`, or `canceled` before it starts.
The server starts events at their start time and ends them at their end time,
or 12 hours after the start without one (checked every 15 seconds).

Gateway (intent `scheduled_events`): `scheduled_event.create`, `.update`
(also on start and end), `.delete` (`{id, space_id}`), `.user_add` and
`.user_remove` (`{event_id, user_id, space_id}`).
Tables `scheduled_events`, `scheduled_event_users` (migration 043, Postgres 028).
