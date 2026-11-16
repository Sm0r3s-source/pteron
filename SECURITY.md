# Security

## Reporting a problem

Please do not open a public issue for a security problem. Use GitHub's private reporting instead:
**Security → Report a vulnerability** on this repository
(<https://github.com/Sm0r3s-source/pteron/security/advisories/new>). Only the author sees it.

Helpful to include: the Pteron build number (Settings → About), Windows or Linux, what a page or a
person could do, and the smallest page or steps that show it.

You will get an answer within a week. Fixes go out as a normal Pteron update; the release notes say
what was fixed once most people have it.

## What is in scope

- Pteron's own code (`ports/servoshell/`), its internal `servo:` pages, the updater, saved passwords,
  downloads, the compatibility tab glue and Plume.
- Pteron's changes to Servo (listed in the README).
- Problems in Servo itself or in a library are welcome too; they are passed on to that project.

## What Pteron does not do (yet)

- Pages run inside Pteron's own process, without a process sandbox. A bug in the engine that a page can
  exploit reaches everything Pteron can reach.
- Revoked certificates are not checked.
- The Windows installer is not code-signed. Updates are: Pteron installs an update only if its SHA-256
  is signed with the author's Ed25519 key, and never goes back to an older build.

## Supported versions

Only the newest build gets fixes. Pteron updates itself from Settings → About.
