import os
import struct
import subprocess
import sys
import ctypes
import signal
import time

def interrupt(_signum, _frame):
    raise KeyboardInterrupt

signal.signal(signal.SIGTERM, interrupt)

def run(argv, pass_fds=()):
    tasks = int(open('/sys/fs/cgroup/pids.current').read())
    visible = sum(name.isdigit() for name in os.listdir('/proc'))
    print(f'gate tasks={tasks} visible={visible} limit=500', flush=True)
    if tasks >= 500 or visible >= 500:
        raise SystemExit('GATE BLOCKED')
    child = subprocess.Popen(argv, pass_fds=pass_fds, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    deadline = time.monotonic() + 20
    peak_tasks, peak_visible = tasks, visible
    try:
        while child.poll() is None:
            tasks = int(open('/sys/fs/cgroup/pids.current').read())
            visible = sum(name.isdigit() for name in os.listdir('/proc'))
            peak_tasks, peak_visible = max(tasks, peak_tasks), max(visible, peak_visible)
            if tasks >= 500 or visible >= 500 or time.monotonic() >= deadline:
                raise RuntimeError(f'GATE OR TIMEOUT tasks={tasks} visible={visible}')
            time.sleep(0.05)
        stdout, stderr = child.communicate()
    except BaseException:
        try: os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError: pass
        child.communicate()
        raise
    print(f'peak tasks={peak_tasks} visible={peak_visible}', flush=True)
    print(f'exit={child.returncode} stdout={stdout.strip()} stderr={stderr.strip()}', flush=True)
    if child.returncode:
        raise SystemExit(child.returncode)

base = '/tmp/attack06-probe-20261108'
if sys.argv[1] == 'compile':
    run(['gcc', '-Wall', '-Wextra', '-O0', '-o', base + '/probe', base + '/probe.c'])
elif sys.argv[1] in ('control', 'layered', 'layered_with_clone3'):
    ep = [101,155,165,166,167,168,169,175,176,246,272,300,303,304,308,310,311,313,320,321,323,328,428,429,430,431,432]
    def ins(code, jt, jf, k): return struct.pack('<HBBI', code, jt, jf, k)
    bpf = ins(0x20,0,0,4) + ins(0x15,1,0,0xc000003e) + ins(0x06,0,0,0x80000000) + ins(0x20,0,0,0)
    for nr in ep: bpf += ins(0x15,0,1,nr) + ins(0x06,0,0,0x50001)
    bpf += ins(0x15,0,1,437) + ins(0x06,0,0,0x50026)
    if sys.argv[1] == 'layered_with_clone3':
        bpf += ins(0x15,0,1,435) + ins(0x06,0,0,0x50026)
    bpf += ins(0x15,0,4,56) + ins(0x20,0,0,16) + ins(0x54,0,0,0x7e020000) + ins(0x15,1,0,0) + ins(0x06,0,0,0x50001) + ins(0x06,0,0,0x7fff0000)
    flags = ['bwrap','--ro-bind','/','/','--unshare-user','--unshare-pid','--unshare-ipc','--unshare-net','--proc','/proc','--cap-drop','ALL']
    runner = [] if sys.argv[1] == 'control' else [sys.executable, __file__, 'outer']
    print('control without product filter', flush=True)
    run(runner + flags + ['--',base+'/probe'])
    fd = os.memfd_create('attack06-control', 0)
    os.write(fd, bpf)
    os.lseek(fd, 0, os.SEEK_SET)
    print(f'control with reconstructed filter mode={sys.argv[1]} bytes={len(bpf)}', flush=True)
    run(runner + flags + ['--seccomp',str(fd),'--',base+'/probe'], (fd,))
    os.close(fd)
elif sys.argv[1] == 'outer':
    class Filter(ctypes.Structure):
        _fields_ = [('code', ctypes.c_ushort), ('jt', ctypes.c_ubyte), ('jf', ctypes.c_ubyte), ('k', ctypes.c_uint)]
    class Program(ctypes.Structure):
        _fields_ = [('length', ctypes.c_ushort), ('filter', ctypes.POINTER(Filter))]
    filters = (Filter * 6)(Filter(0x20,0,0,0), Filter(0x15,0,1,435), Filter(0x06,0,0,0x50016), Filter(0x15,0,1,437), Filter(0x06,0,0,0x50016), Filter(0x06,0,0,0x7fff0000))
    program = Program(len(filters), filters)
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(38, 1, 0, 0, 0) != 0 or libc.prctl(22, 2, ctypes.byref(program), 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'synthetic outer seccomp setup failed')
    os.execvp(sys.argv[2], sys.argv[2:])
