# Welcome screen

`GET /spaces/{space_id}/welcome-screen` (members) returns
`{enabled, description, channels: [{channel_id, description, emoji?}]}`
(channels deleted since are left out). `PATCH` (with `manage_space`) replaces the
fields given: a description of up to 140 characters, up to 5 channels of the
space without repeats, each with a line of 1–50 characters and an optional emoji.
`welcome_screen.update` goes to the space. Clients show it to a member on their
first visit. Table `space_welcome_screens` (migration 046, Postgres 031).
