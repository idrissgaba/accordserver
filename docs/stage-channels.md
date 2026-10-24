# Stage channels

A stage is a voice channel (`"type": "stage"`) with an audience. It builds on
[voice moderation](voice-moderation.md): being in the audience is enforced in
LiveKit exactly like a missing `speak` right.

- Everyone joins in the audience: `suppress: true`, no microphone, camera or
  screen. Rejoining starts in the audience again.
- Stage moderators are members with `mute_members` on the channel.
- The channel `topic` is the stage topic.

## Endpoint

`PATCH /spaces/{space_id}/voice-states/{user_id}` (`@me` for yourself), body:

| Field | Meaning |
| --- | --- |
| `request_to_speak` | `true` raises your hand (needs `speak`), `false` lowers it. Yourself only. |
| `suppress` | `false` brings the member up to speak, `true` sends them back to the audience. Moderators only, except that anyone may send themselves back. |

Returns `{ "data": <voice state> }`. A raised hand shows as
`request_to_speak_timestamp` on the voice state and is announced with
`voice.state_update`; the field is omitted when there is none, so clients that
don't know stages see the same payload as before. Speaking or returning to the
audience lowers the hand.

Errors: `400 channel_not_stage`, `403` without the right, `404` when the user is
not in voice in that space.
