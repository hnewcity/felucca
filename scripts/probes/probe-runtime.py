# Local audit probes; uses only temporary configs and loopback listeners.
import json, pathlib, signal, socket, struct, subprocess, tempfile, time
root=pathlib.Path(__file__).resolve().parents[2]
(root/'target/tmp').mkdir(parents=True, exist_ok=True)
server=root/'target/release/felucca-server'
agent=root/'target/release/felucca-agent'
results={}
def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1',0)); return s.getsockname()[1]
def read_exact(s,n):
    out=b''
    while len(out)<n:
        data=s.recv(n-len(out))
        if not data: break
        out+=data
    return out
with tempfile.TemporaryDirectory(prefix='audit-',dir=root/'target/tmp') as temp:
    d=pathlib.Path(temp)
    bridge,forward,socks=port(),port(),port()
    backend=socket.socket(); backend.bind(('127.0.0.1',0)); backend.listen(); backend.settimeout(3)
    backend_port=backend.getsockname()[1]
    cfg=d/'server.toml'; cfg.write_text(f'''[bridge]
bind='127.0.0.1:{bridge}'
[client.audit]
vkey='audit-only-secret'
allow_config_push=true
max_connections=1
''')
    acfg=d/'agent.toml'; acfg.write_text(f'''server='127.0.0.1:{bridge}'
vkey='audit-only-secret'
key_id='audit'
transport='tcp'
reconnect_min='100ms'
reconnect_max='200ms'
[[tunnel]]
mode='tcp'
port={forward}
bind='127.0.0.1'
[[tunnel.backend]]
addr='127.0.0.1:{backend_port}'
[[tunnel]]
mode='socks5'
port={socks}
bind='127.0.0.1'
''')
    procs=[]; handles=[]; target=None
    try:
        for binary,config,log in [(server,cfg,d/'server.log'),(agent,acfg,d/'agent.log')]:
            stream=log.open('w'); handles.append(stream)
            procs.append(subprocess.Popen([str(binary),'--config',str(config)],cwd=root,stdout=stream,stderr=stream))
        deadline=time.monotonic()+6
        while 'tunnels registered' not in (d/'agent.log').read_text():
            assert all(p.poll() is None for p in procs), 'process exited'
            if time.monotonic()>deadline: raise RuntimeError((d/'agent.log').read_text())
            time.sleep(.02)
        with socket.create_connection(('127.0.0.1',socks),timeout=2) as s:
            s.sendall(bytes([5,1,2])); results['socks5_user_password_method_reply']=read_exact(s,2).hex()
        time.sleep(.1)
        with socket.create_connection(('127.0.0.1',socks),timeout=2) as s:
            s.sendall(bytes([5,1,0])); assert read_exact(s,2)==bytes([5,0])
            s.sendall(bytes([5,3,0,1,0,0,0,0,0,0])); results['socks5_udp_associate_reply']=read_exact(s,10).hex()
        time.sleep(.1)
        client=socket.create_connection(('127.0.0.1',forward),timeout=2)
        target,_=backend.accept(); target.settimeout(2)
        client.sendall(b'probe')
        assert read_exact(target,5)==b'probe'
        client.setsockopt(socket.SOL_SOCKET,socket.SO_LINGER,struct.pack('ii',1,0)); client.close()
        try:
            data=target.recv(1)
            results['backend_after_public_rst']='EOF' if data==b'' else data.hex()
        except socket.timeout:
            results['backend_after_public_rst']='still open after 2 seconds'
        with socket.create_connection(('127.0.0.1',forward),timeout=2) as second:
            try:
                second.sendall(b'second')
                data=second.recv(1)
                results['new_connection_with_cap_one']='closed' if not data else data.hex()
            except ConnectionResetError:
                results['new_connection_with_cap_one']='reset'
            except socket.timeout:
                results['new_connection_with_cap_one']='open without reply'
        backend.settimeout(.2)
        try:
            more,_=backend.accept(); more.close()
            results['second_request_reached_backend']=True
        except socket.timeout:
            results['second_request_reached_backend']=False
    finally:
        if target: target.close()
        backend.close()
        for p in procs:
            if p.poll() is None: p.send_signal(signal.SIGTERM)
        for p in procs:
            try: p.wait(timeout=3)
            except subprocess.TimeoutExpired: p.kill(); p.wait()
        for stream in handles: stream.close()
    shared=port()
    cfg.write_text(f'''[bridge]
bind='127.0.0.1:{shared}'
[http]
bind='127.0.0.1:{shared}'
tls_bind=''
[client.audit]
vkey='audit-only-secret'
allow_config_push=true
''')
    p=subprocess.run([str(server),'--config',str(cfg)],cwd=root,capture_output=True,text=True,timeout=3)
    results['bridge_http_same_port_exit']=p.returncode
    results['bridge_http_same_port_error']=p.stderr.splitlines()[-1]
print(json.dumps(results,indent=2,ensure_ascii=False))
