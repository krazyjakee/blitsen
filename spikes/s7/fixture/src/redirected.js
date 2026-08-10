// Vite checks that static import targets exist before the dev-server middleware
// can issue the redirect used by this spike. This file should never be served.
throw new Error("redirect middleware was bypassed");
