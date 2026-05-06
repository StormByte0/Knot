// TODO: esbuild config for server bundle
// Entry: src/server.ts → out/src/server.js
// External: vscode
// Format: cjs, platform: node

const esbuild = require('esbuild');

const isWatch = process.argv.includes('--watch');

const buildOpts = {
  entryPoints: ['src/server.ts'],
  bundle: true,
  outfile: 'out/src/server.js',
  external: ['vscode'],
  format: 'cjs',
  platform: 'node',
  sourcemap: true,
};

if (isWatch) {
  esbuild.context(buildOpts).then(ctx => ctx.watch());
} else {
  esbuild.build(buildOpts).then(() => {
    console.log('Server build complete.');
  });
}
