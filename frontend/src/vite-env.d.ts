/// <reference types="vite/client" />

// ELK ships its in-thread engine as an untyped CommonJS script; graph-layout.ts
// narrows the module shape before use.
declare module "elkjs/lib/elk-worker.min.js";
