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

### Registration with an invite (`invite_only`)

Upstream's `invite_only` policy refuses every registration and admins cannot
create accounts. Here `POST /api/v1/auth/register` accepts an optional
`invite_code`; under `invite_only` it must be redeemable (exists, unexpired,
uses left). The invite is redeemed before the default-space auto-join (so an
invite to that space still counts a use) and the account is removed again if it
loses the race for the last use. Compatible with official clients: they don't
send the field, which keeps upstream's behaviour.

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
