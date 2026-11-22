// Independent UDP receiver for burst probes. Sharing the sender's event loop
// prevents the receiver from draining its socket during a synchronous burst.
import { createSocket } from 'node:dgram';
import { parentPort, workerData } from 'node:worker_threads';

const counters = new Int32Array(workerData.counters);
const socket = createSocket('udp4');
socket.on('error', error => { throw error; });
socket.on('message', (message, peer) => {
  Atomics.add(counters, 0, 1);
  socket.send(message, peer.port, peer.address, error => {
    if (error) Atomics.add(counters, 1, 1);
  });
});
socket.bind(0, '127.0.0.1', () => {
  const defaultReceiveBuffer = socket.getRecvBufferSize();
  if (workerData.receiveBuffer) socket.setRecvBufferSize(workerData.receiveBuffer);
  parentPort.postMessage({ port: socket.address().port, defaultReceiveBuffer, receiveBuffer: socket.getRecvBufferSize() });
});
