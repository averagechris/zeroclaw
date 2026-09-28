# Launch sequence

Type: grilling
Status: resolved
Blocked by: none

## Question

Should Kagi web search be included in the initial no-tool baseline, or added
immediately after the baseline has its approved-user access, one-bot DM/group
routing, separate DM sessions, shared group session, and permission boundaries
working?

## Answer

First get a text-only bot running with approved-user access, one-bot
DM/group routing, separate DM sessions, a shared group session, and permission
boundaries working. Its only tools are the built-in memory tools, which also
exercise memory isolation between agents (see [Memory
tools](21-memory-tools.md)); this replaced the earlier "no tools" baseline on
2026-09-27. Then add plain Kagi web search immediately as the first
shared tool. Only after the owner-route isolation tests pass should the
owner's private agent get unprompted shell; partner and household agents never
get it. Other tool ideas and account integrations remain deferred.

## Artifacts

None.
