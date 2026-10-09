# Security policy

## Supported versions

Security fixes are released for the latest version only.

## Reporting a vulnerability

Please do not report security vulnerabilities in public issues.

Report them privately through GitHub instead: open the repository's **Security**
tab and choose **Report a vulnerability**
([direct link](https://github.com/metinata/caffeinate-win/security/advisories/new)).

Please include what you found, the steps to reproduce it, and the impact you expect.
You will get a reply as soon as possible and will be kept informed while a fix is
prepared. The published advisory will credit you, unless you prefer otherwise.

Examples of security issues in caffeinate:

- starting a different program than the one requested, for example through the
  `PATH`/`PATHEXT` lookup
- unsafe argument handling when starting `.cmd` or `.bat` files
- leaving the system in a changed state after caffeinate exits
- the install script installing anything other than the release's files
