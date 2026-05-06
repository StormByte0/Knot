// TODO: esbuild config for client bundle
// Entry: src/extension.ts → out/extension.js
// External: vscode
// Format: cjs, platform: node

const esbuild = require('esbuild');

const isWatch = process.argv.includes('--watch');

const buildOpts = {
  entryPoints: ['src/extension.ts'],
  bundle: true,
  outfile: 'out/extension.js',
  external: ['vscode'],
  format: 'cjs',
  platform: 'node',
  sourcemap: true,
};

if (isWatch) {
  esbuild.context(buildOpts).then(ctx => ctx.watch());
} else {
  esbuild.build(buildOpts).then(() => {
    console.log('Client build complete.');
  });
}
