# What macOS needs to be told

`Info.plist` holds the two sentences macOS shows an operator when PushOS first
asks for the microphone and for speech recognition. A command line tool has no
bundle to keep them in, so each build script links the file straight into its
binary's `__TEXT,__info_plist` section, which is what a bundle would have
amounted to.

Without it the first held control fails with an opaque number instead of a
prompt, and no amount of System Settings would fix it.

Kept here rather than in a crate because two crates need the same words: the
`pushos` binary, and the `transcribe` example that checks an engine without a
Push 2 attached.
