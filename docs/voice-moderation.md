# Voice moderation

Voice rights are enforced in LiveKit, not trusted to the client.

## What a participant may do

Each voice session has a media policy, computed from:

| Source | Effect |
|---|---|
| Member `mute` (set by someone with `mute_members`) | No microphone |
| Member `deaf` (set by someone with `deafen_members`) | No microphone, and no subscriptions (hears and sees nothing) |
| Channel denies `speak` | No microphone; the voice state shows `suppress: true` |
| Channel denies `stream` | No camera or screen share |

DM and group DM calls are never restricted.

The policy is written into the LiveKit join token (`canPublishSources`,
`canPublish`, `canSubscribe`) and into the voice state (`mute`, `deaf`,
`suppress`). When it changes during a call (a moderator mutes someone, a role
or channel overwrite changes), the server pushes the new permission to the
live participant with LiveKit's `UpdateParticipant`; LiveKit then unpublishes
tracks that are no longer allowed. The periodic voice reconciliation retries
an update LiveKit could not apply yet (participant not connected).

## Moving and disconnecting members

`PATCH /spaces/{space_id}/members/{user_id}` accepts `channel_id`
(requires `move_members`):

- a voice channel id of the same space moves the member there. The mover needs
  `connect` on that channel, and so does the member. The member must already be
  in voice in this space (`400` otherwise). Their sessions receive a
  `voice.server_update` for the new room with `"moved": true` and
  `"from_channel_id"`; a client that understands it reconnects to the new
  room. Other clients simply see the member's voice state change channel.
- `null` disconnects the member from voice.

## User limits

A voice channel's `user_limit` (when above 0) is enforced on join, over REST
and over the gateway: a join into a full channel fails with `403 channel_full`.
Members with `move_members` may still join, and rejoining your own seat is not
blocked.
