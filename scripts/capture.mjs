import {once} from 'node:events';
const [endpoint, seconds] = process.argv.slice(1);
const ws = new WebSocket(endpoint);
await once(ws, 'open');
let id = 0, origin, pending = new Map(), failure;
const send = (method, params = {}) => new Promise((resolve, reject) => {
  pending.set(++id, {resolve, reject});
  ws.send(JSON.stringify({id, method, params}));
});
ws.addEventListener('message', event => {
  try {
    const msg = JSON.parse(event.data);
    if (msg.id) {
      const p = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) p?.reject(Error(JSON.stringify(msg.error))); else p?.resolve(msg.result);
    }
    if (msg.method === 'Page.screencastFrame') {
      const {data, metadata, sessionId} = msg.params;
      origin ??= performance.now();
      process.stdout.write(JSON.stringify({data, timestamp: metadata.timestamp, received: performance.now() - origin}) + '\n');
      send('Page.screencastFrameAck', {sessionId}).catch(e => {failure = e;});
    }
  } catch(e) {failure = e;}
});
ws.addEventListener('close', () => {
  for (const p of pending.values()) p.reject(Error('CDP connection closed'));
  pending.clear();
});
await send('Page.enable');
await send('Page.startScreencast', {format: 'jpeg', quality: 85, everyNthFrame: 1});
await new Promise(resolve => setTimeout(resolve, Number(seconds) * 1000));
await send('Page.stopScreencast');
if (failure) throw failure;
if (origin === undefined) throw Error('No frames captured');
process.stdout.write(JSON.stringify({end: (performance.now() - origin) / 1000}) + '\n');
ws.close();
