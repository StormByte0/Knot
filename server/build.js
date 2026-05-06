// TODO: esbuild config for server bundle
// Entry: src/server.ts → out/src/server.js
// External: vscode
// Format: cjs, platform: node

const path = require('path');
const esbuild = require('esbuild');

const isWatch = process.argv.includes('--watch');

const buildOpts = {
  entryPoints: [path.join(__dirname, 'src/server.ts')],
  bundle: true,
  outfile: path.join(__dirname, 'out/src/server.js'),
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
