#!/bin/bash

# . ./script/env.sh
# . ./script/config.sh

# Flags for every server, set by the driver that sources this: the ones the
# servers define, such as -nodelay, with the benchmark client's own filtered
# out. Read from a variable rather than from "$@" because `source file` with no
# arguments leaves the caller's positional parameters in place, which is how
# the client's flags would otherwise reach the servers.
if [ -z "${server_flags+set}" ]; then
    # Invoked directly rather than sourced: take our own arguments.
    server_flags="$*"
fi

# Every server at once, for a server node (BENCH_ROLE=server): the client
# node cannot start or stop them, so they all stay up for the whole run. A
# single-node run starts each one for its own turn instead; see start_server
# in script/env.sh.
for f in ${frameworks[@]}; do
    echo
    ./script/server.sh $f $server_flags
done
