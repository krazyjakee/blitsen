import { mkdtemp, rm, mkdir, readFile, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { linkBundle } from '../src/bundle.mjs';
import { buildRuntime, repository, capture } from './build-addon.mjs';
import { cargoTargetDirectory } from '../src/android-toolchain.mjs';

// Exercise the same packaged export with both supported driver hosts. The
// application always runs in its own runtime; the driver never owns its DOM.
buildRuntime({ release: false });
const target = await cargoTargetDirectory(repository, cmd => capture(cmd, { cwd: repository }));
const scratch = await mkdtemp(join(tmpdir(), 'blitsen-ui-'));
try {
  const install = Bun.spawnSync({ cmd: [process.execPath, 'install', '--frozen-lockfile'],
    cwd: join(repository, 'examples/vite-react'), stdout: 'inherit', stderr: 'inherit' });
  if (install.exitCode) throw new Error('Could not install the React acceptance fixture');
  const compiled = await Bun.build({ entrypoints: [join(import.meta.dir, 'fixtures/ui/app.jsx')],
    target: 'browser', format: 'esm',
    // Reuse the pinned React acceptance example without adding dependencies to
    // the public package just to test it.
    plugins: [{ name: 'fixture-react', setup(build) {
      build.onResolve({ filter: /^react(?:-dom)?(?:\/.*)?$/ }, args => ({
        path: import.meta.resolveSync(args.path, join(repository, 'examples/vite-react/src/main.jsx')),
      }));
    }}],
  });
  if (!compiled.success) throw new AggregateError(compiled.logs, 'Fixture bundle failed');
  const files = new Map([
    ['index.html', Buffer.from('<!doctype html><style>body{margin:0}.app{display:flex;min-height:100vh}button{height:40px}input{height:32px}</style><div id="root"></div><script type="module" src="app.js"></script>')],
    ['app.js', Buffer.from(await compiled.outputs[0].text())],
  ]);
  const output = join(scratch, process.platform === 'win32' ? 'UI.exe' : 'UI');
  await linkBundle({ runtime: join(target, 'debug', process.platform === 'win32' ? 'blitsen-runtime.exe' : 'blitsen-runtime'), output, files });
  for (const host of ['node', process.execPath]) {
    const artifacts = join(scratch, host === 'node' ? 'node' : 'bun');
    await mkdir(artifacts);
    const result = Bun.spawnSync({ cmd: [host, join(import.meta.dir, 'run-ui-driver.mjs'), output, artifacts],
      stdout: 'inherit', stderr: 'inherit', env: { ...process.env, XDG_DATA_HOME: join(scratch, 'data') } });
    if (result.exitCode) throw new Error(`${host} UI driver failed`);
    const failure = (await readdir(artifacts)).find(name => name.endsWith('-click-failed.json'));
    const evidence = JSON.parse(await readFile(join(artifacts, failure), 'utf8'));
    if (!Array.isArray(evidence.events)) throw new Error('Missing event evidence');
  }
} finally { await rm(scratch, { recursive: true, force: true }); }
