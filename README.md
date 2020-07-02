This is a reproducer for https://github.com/libp2p/rust-libp2p/issues/1629

In the dependencies, I let my fork with more debug logs. Feel free to use the regular libp2p project. My fork has a panic!() when the handler times out. It's easier to spot it when it happens.

I can't reproduce outside of release mode.

To start it, clear anything listening on ports 4040 / 4041 or change both the PORT environment variable and the BOOTSTRAP_SERVERS multiaddr:
- Open 3 terminals
- In terminal one, start: `PRIVATE_KEY_PATH=./p2p-private.pk8 PORT=4040 RUST_LOG=info,libp2p_reproducer=debug cargo run --release`
- In terminal two, start: `BOOTSTRAP_SERVERS="QmTZWrWZzN7i4h2tGAXFcwkhbaArjPK1BzcF2rFMehkMb7:/ip4/127.0.0.1/tcp/4040" PORT=4041 RUST_LOG=info,libp2p_reproducer=debug,libp2p_mplex=trace cargo run --release 2>/tmp/debug-repro2.txt` (notice the logs redirection)
- In terminal three, start: `tail -f /tmp/debug-repro2.txt`

The log redirection seems to be needed as soon as you start to log a bit too much. For example, without the `libp2p_mpex=trace` log, the redirection is not needed. I have the same issue on my real project, the bug triggers more often with the redirection.

Wait a few seconds for the DHT bootstrap and then in terminal two, type `POPULATE` and then Enter.

From there, it starts to create 1500 random providers records in the DHT. At some point, if you used my fork, a panic on terminal 2 will appear when the upgrade times out. If you do not use my fork, follow the `remaining={}` log to know how many records are yet to be inserted. If it's not 0 and it stops, it may be the same bug. Wait 10 secs and if it continues, then it's most probably the bug. No error will be returned by kademlia so none of the panic I've put will be hit.

If you can't make it happen with the initial try, type `POPULATE` again and again until it does. On my computer, it usually triggers on the first try.