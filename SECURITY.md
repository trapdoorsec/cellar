# Security Policy

## Supported Versions

Only the latest release of cellar receives security updates.

## Reporting a Vulnerability

**Do not report security vulnerabilities through public GitHub issues.**

Instead, use GitHub's private vulnerability reporting:

1. Go to [github.com/trapdoorsec/cellar/security/advisories](https://github.com/trapdoorsec/cellar/security/advisories)
2. Click **"Report a vulnerability"**
3. Fill in the details

You can expect an initial response within 5 business days. Please include:

- A description of the vulnerability
- Steps to reproduce or a proof of concept
- The affected version(s), if known
- Any potential impact assessment

## Disclosure Policy

We follow coordinated (responsible) disclosure:

- Reports are acknowledged within 5 business days.
- We will investigate and develop a fix.
- The vulnerability is not publicly disclosed until a fix has been released.
- Public disclosure happens with the reporter's agreement, or after 90 days from acknowledgment — whichever comes first.
- Credit is given to the reporter in the advisory unless they request otherwise.

## Scope

- **In scope:** Vulnerabilities in the cellar application itself, including its ISO 9660/Joliet generation, file handling, and UI, build process and github actions.
- **Out of scope:** Vulnerabilities in third-party dependencies (report these upstream), issues in the operating system or runtime environment, and denial-of-service attacks requiring physical access.

## No Bounty

Sorry, I am a solo developer on this project so I can't afford a bounty program. But as a bounty hunter myself, I do appreciate your work. The best way I can show that is through giving you credit for any legitimate, in scope findings 
