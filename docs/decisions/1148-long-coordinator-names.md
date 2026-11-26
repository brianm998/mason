# 1148: Shorten a long coordinator name with a hash instead of refusing it

## Context

A node keeps at most 60 characters of a coordinator's name. The coordinator sends its host name, which can be longer, and the node cut it without saying so. Lookups by the full name then failed.

## Decision

One function, `node_serve::coordinator_name`, gives the name: the trimmed name if it fits in 60 characters, else its first 51 characters, a `-`, and 8 hex digits of the SHA-256 of the whole name. The coordinator sends that; the node applies it again when it stores a name and when it looks one up. It leaves a name that fits unchanged.

The node applies it too so that a coordinator that still sends its full name is stored under the same name, and the owner can remove it by the full host name.

## Alternative

Refuse an over-long name with a 400. Not chosen: the coordinator's name is its host name, which the owner would have to change to pair at all.

## Consequence

A long name is listed in its shortened form. Names stored cut short before this change keep that form until the coordinator pairs again.
