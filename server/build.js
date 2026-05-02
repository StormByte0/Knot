const { build } = require('esbuild');
const path = require('path');

build({
  entryPoints: [path.join(__dirname, 'src/lspServer.ts')],
  bundle: true,
  outfile: path.join(__dirname, 'out/src/lspServer.js'),
  external: ['vscode'],
  format: 'cjs',
  platform: 'node',
  sourcemap: true,
}).catch(() => process.exit(1));