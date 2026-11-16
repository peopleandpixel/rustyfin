// The Jellyfin Android app recognises a Jellyfin web client by a classic script named main.<name>.bundle.js:
// it answers that request with its own loader, which adds window.NativeShell and then loads this file again
// with ?deferred=true. The client itself is app.js; modules/nativeshell.js picks the bridge up. In a
// browser this file does nothing.
