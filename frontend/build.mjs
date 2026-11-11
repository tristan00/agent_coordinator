import { build } from "esbuild";
await build({entryPoints:["src/metrics.tsx"],outfile:"../web/metrics.js",bundle:true,minify:true,format:"esm",jsx:"automatic",alias:{"@":"./src"},define:{"process.env.NODE_ENV":'"production"'},legalComments:"linked"});
