#!/usr/bin/env node
// The persistent JavaScript namespace.
//
// Same contract as bootstrap.py: one process, one namespace, one Unix socket,
// newline-delimited JSON in and out. Node is here so that a project whose data
// wrangling is already JavaScript does not have to switch languages to keep a
// variable alive between tool calls.

'use strict';

const fs = require('fs');
const net = require('net');
const vm = require('vm');

const args = process.argv.slice(2);
const socketPath = args[args.indexOf('--socket') + 1];
const idleSeconds = Number(args[args.indexOf('--idle') + 1] || 3600);

// The namespace. `require` is exposed deliberately: a kernel that cannot read a
// file is not much of a kernel.
const context = vm.createContext({
  require,
  console,
  process,
  Buffer,
  setTimeout,
  clearTimeout,
});

const RESERVED = new Set(['require', 'console', 'process', 'Buffer', 'setTimeout', 'clearTimeout']);

function summarise(name, value) {
  const info = { name, type_name: typeof value, length: null, size_bytes: null, preview: null };

  if (value !== null && value !== undefined) {
    if (Array.isArray(value)) {
      info.type_name = 'Array';
      info.length = value.length;
    } else if (typeof value === 'string') {
      info.length = value.length;
    } else if (value instanceof Map || value instanceof Set) {
      info.type_name = value.constructor.name;
      info.length = value.size;
    } else if (typeof value === 'object') {
      info.type_name = value.constructor ? value.constructor.name : 'Object';
      info.length = Object.keys(value).length;
    }
  }

  try {
    const text = typeof value === 'string' ? JSON.stringify(value) : String(value);
    info.size_bytes = Buffer.byteLength(text || '');
    info.preview = text && text.length > 96 ? `${text.slice(0, 93)}...` : text;
  } catch (error) {
    info.preview = '<unstringifiable>';
  }

  return info;
}

function execute(code, timeoutSeconds) {
  const started = Date.now();
  let stdout = '';
  let stderr = '';
  let result = null;
  let error = null;
  let timedOut = false;

  const writeOut = process.stdout.write.bind(process.stdout);
  const writeErr = process.stderr.write.bind(process.stderr);
  process.stdout.write = (chunk) => { stdout += chunk; return true; };
  process.stderr.write = (chunk) => { stderr += chunk; return true; };

  try {
    const value = vm.runInContext(code, context, {
      filename: '<pa>',
      timeout: Math.max(1, timeoutSeconds) * 1000,
    });
    if (value !== undefined) {
      result = typeof value === 'string' ? value : JSON.stringify(value) ?? String(value);
    }
  } catch (thrown) {
    if (thrown && /Script execution timed out/.test(String(thrown.message))) {
      timedOut = true;
      error = `timed out after ${timeoutSeconds}s`;
    } else {
      error = thrown && thrown.stack ? thrown.stack : String(thrown);
    }
  } finally {
    process.stdout.write = writeOut;
    process.stderr.write = writeErr;
  }

  return {
    ok: error === null,
    stdout,
    stderr,
    result,
    error,
    duration_ms: Date.now() - started,
    timed_out: timedOut,
  };
}

function handle(request) {
  switch (request.op) {
    case 'ping':
      return { ok: true, pid: process.pid };
    case 'exec':
      return execute(request.code || '', request.timeout || 120);
    case 'vars':
      return {
        ok: true,
        vars: Object.keys(context)
          .filter((name) => !RESERVED.has(name))
          .map((name) => summarise(name, context[name])),
      };
    case 'reset':
      Object.keys(context)
        .filter((name) => !RESERVED.has(name))
        .forEach((name) => { delete context[name]; });
      return { ok: true };
    case 'snapshot': {
      // Only JSON-serialisable bindings survive, and the response says which
      // ones did not rather than pretending the snapshot is complete.
      const saved = {};
      const skipped = [];
      for (const name of Object.keys(context)) {
        if (RESERVED.has(name)) continue;
        try {
          JSON.stringify(context[name]);
          saved[name] = context[name];
        } catch (error) {
          skipped.push(name);
        }
      }
      fs.writeFileSync(request.path, JSON.stringify(saved));
      return { ok: true, saved: Object.keys(saved).sort(), skipped: skipped.sort(), path: request.path };
    }
    case 'restore': {
      if (!fs.existsSync(request.path)) {
        return { ok: false, error: `no snapshot at ${request.path}`, restored: [] };
      }
      Object.assign(context, JSON.parse(fs.readFileSync(request.path, 'utf8')));
      return { ok: true, restored: Object.keys(context).filter((n) => !RESERVED.has(n)).sort() };
    }
    case 'shutdown':
      return { ok: true };
    default:
      return { ok: false, error: `unknown op '${request.op}'` };
  }
}

if (fs.existsSync(socketPath)) fs.unlinkSync(socketPath);

const server = net.createServer((conn) => {
  let buffer = '';
  conn.on('data', (chunk) => {
    buffer += chunk;
    const newline = buffer.indexOf('\n');
    if (newline === -1) return;

    let request = {};
    let response;
    try {
      request = JSON.parse(buffer.slice(0, newline));
      response = handle(request);
    } catch (error) {
      response = { ok: false, error: String(error) };
    }

    conn.end(`${JSON.stringify(response)}\n`, () => {
      if (request.op === 'shutdown') {
        server.close();
        process.exit(0);
      }
    });
  });
});

let idleTimer = null;
function resetIdle() {
  if (idleTimer) clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    // Nobody has used this namespace in a long time; free the memory.
    server.close();
    process.exit(0);
  }, idleSeconds * 1000);
  idleTimer.unref();
}

server.on('connection', resetIdle);
server.listen(socketPath, () => {
  fs.chmodSync(socketPath, 0o600);
  resetIdle();
});
