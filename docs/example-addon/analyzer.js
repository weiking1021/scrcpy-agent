// A program that comes with the example add-on (it is not part of the add-on's script: `main.js` starts it with
// `host.process.spawn`, found through `host.addon.folder()`). It runs in Node, reads the H.264 stream from its
// input, counts the bytes and the key frames, and writes one line a second, which the add-on logs.
let bytes = 0;
let keyFrames = 0;
let tail = Buffer.alloc(0);
const report = () => console.log(JSON.stringify({ bytes, keyFrames }));
setInterval(report, 1000);
process.stdin.on("data", (chunk) => {
  bytes += chunk.length;
  const data = Buffer.concat([tail, chunk]);
  // A key frame is the start code 00 00 01 followed by a NAL unit of type 5.
  for (let i = 0; i + 3 < data.length; i++) {
    if (data[i] === 0 && data[i + 1] === 0 && data[i + 2] === 1 && (data[i + 3] & 0x1f) === 5) keyFrames++;
  }
  tail = data.subarray(Math.max(0, data.length - 3));
});
process.stdin.on("end", () => {
  report();
  process.exit(0);
});
