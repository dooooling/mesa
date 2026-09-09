// Sharp7 interop harness：独立 NCK 客户端（未改上游源码）对打 Mesa emulator。
//
// 架构（本进程内）：
//   Sharp7 S7Client → tap(127.0.0.1:T, 记录 raw) → emulator(127.0.0.1:E)
// tap 按 TPKT 长度逐包记录：req-NN.bin（client→server）、rsp-NN.bin（server→client）。
// 另写 manifest.json（参数、返回码、解码字节数），供 Rust 回放测试与人工核对。
//
// 约束：Area 恒为 0（N），避开 <<4/<<5 未决分歧（0<<4 == 0<<5）。
using System;
using System.Collections.Generic;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using System.Threading;
using Sharp7;

static class Harness
{
    sealed class Args
    {
        public string Emulator = "127.0.0.1:1102";
        public string Out = "vectors";
        public int Unit = 1;
        public int Module = 18;
        public int Param = 42;
        public int Start = 0;
        public int Amount = 1;
        public int WordLen = 2; // S7WLByte
    }

    static int Main(string[] argv)
    {
        var a = Parse(argv);
        Directory.CreateDirectory(a.Out);

        var tap = new TcpListener(IPAddress.Loopback, 0);
        tap.Start();
        int tapPort = ((IPEndPoint)tap.LocalEndpoint).Port;
        Console.WriteLine($"tap listening on 127.0.0.1:{tapPort}");

        var emuEp = ParseEp(a.Emulator);
        using var toEmu = new TcpClient();
        toEmu.Connect(emuEp); // 先接通 emulator，tap 再接受 client（顺序固定，无竞态）

        var acceptDone = new ManualResetEventSlim(false);
        TcpClient fromClient = null;
        var acceptThread = new Thread(() =>
        {
            // Sharp7 建连前会打一个一次性 TCPPing 连接（无数据即关）：
            // 只选第一个携带数据的连接，ping socket 直接丢弃。
            try { fromClient = AcceptDataConnection(tap, 10000); }
            catch (Exception e) { Console.WriteLine($"accept end: {e.Message}"); }
            acceptDone.Set();
        });
        acceptThread.IsBackground = true;
        acceptThread.Start();

        var events = new List<object>();
        var seq = new Seq();

        // client 与 pump 并发：握手包必须在 NckConnectTo 期间就被转发。
        // 顺序：等 accept（client TCP 经 backlog 完成）→ 起 pump → client 建连+读写。
        var clientDone = new ManualResetEventSlim(false);
        int clientRc = 0;
        var clientThread = new Thread(() =>
        {
            try
            {
                var client = new S7Client();
                client.PLCPort = tapPort;
                int rc = client.NckConnectTo("127.0.0.1");
                Console.WriteLine($"NckConnectTo rc={rc}");
                if (rc != 0) { clientRc = 1; return; }

                // --- single read（ReadNckArea） ---
                byte[] singleBuf = new byte[256];
                int bytesRead = 0;
                rc = client.ReadNckArea(0, a.Unit, a.Module, a.Param, a.Start, a.Amount, a.WordLen, singleBuf, ref bytesRead);
                Console.WriteLine($"ReadNckArea rc={rc} bytesRead={bytesRead} data={BitConverter.ToString(singleBuf, 0, Math.Max(bytesRead, 0))}");
                lock (events) { events.Add(new { op = "single", rc, bytesRead, data = BitConverter.ToString(singleBuf, 0, Math.Max(bytesRead, 0)) }); }

                // --- multi read（ReadMultiNckVars，两项：param 与 param+1） ---
                var multi = new S7NckMultiVar(client);
                byte[] buf0 = new byte[256];
                byte[] buf1 = new byte[256];
                multi.NckAdd(0, a.Unit, a.Module, a.Param, a.WordLen, a.Start, a.Amount, ref buf0);
                multi.NckAdd(0, a.Unit, a.Module, a.Param + 1, a.WordLen, a.Start, a.Amount, ref buf1);
                rc = multi.ReadNck();
                Console.WriteLine($"MultiRead rc={rc} results=[{multi.Results[0]},{multi.Results[1]}]");
                lock (events) { events.Add(new { op = "multi", rc, results = new[] { multi.Results[0], multi.Results[1] } }); }
                if (rc != 0) clientRc = 2;

                client.Disconnect();
            }
            catch (Exception e) { Console.WriteLine($"client end: {e.Message}"); clientRc = 1; }
            clientDone.Set();
        });
        clientThread.IsBackground = true;
        clientThread.Start();

        if (!acceptDone.Wait(10000)) return Fail("tap accept timeout", -1);
        // tap 双向泵跑在独立线程（req = client→server，rsp = 反向）。
        // accept 已发生，client 握手字节已在内核缓冲，pump 接管即转发，不丢包。
        var pumpThread = new Thread(() =>
        {
            try { Pump(fromClient.GetStream(), toEmu.GetStream(), a.Out, seq); }
            catch (Exception e) { Console.WriteLine($"pump end: {e.Message}"); }
        });
        pumpThread.IsBackground = true;
        pumpThread.Start();
        clientDone.Wait(60000);
        // 确定性 teardown：关两端 socket 解开 pump 的阻塞读，再 Join。
        try { fromClient.Close(); } catch { }
        try { toEmu.Close(); } catch { }
        pumpThread.Join(5000);
        tap.Stop();

        var manifest = new
        {
            sharp7_commit = "eac1e728f8523278564e83c276fa6b8d281e6ba0",
            area = 0,
            unit = a.Unit,
            module = a.Module,
            param = a.Param,
            start = a.Start,
            amount = a.Amount,
            wordLen = a.WordLen,
            files = seq.n,
            events,
        };
        File.WriteAllText(Path.Combine(a.Out, "manifest.json"),
            JsonSerializer.Serialize(manifest, new JsonSerializerOptions { WriteIndented = true }));
        Console.WriteLine($"wrote {seq.n} packet files + manifest.json to {a.Out}");
        return clientRc;
    }

    sealed class Seq { public int n; }

    // 一次读一整包 TPKT（4 字节头给长度），双向各自编号落盘。
    static void Pump(NetworkStream c2s, NetworkStream s2c, string outDir, Seq seq)
    {
        var t1 = new Thread(() => Forward(c2s, s2c, outDir, "req", seq));
        var t2 = new Thread(() => Forward(s2c, c2s, outDir, "rsp", seq));
        t1.IsBackground = t2.IsBackground = true;
        t1.Start(); t2.Start();
        t1.Join(); t2.Join();
    }

    static void Forward(NetworkStream from, NetworkStream to, string outDir, string tag, Seq seq)
    {
        try
        {
            while (true)
            {
                byte[] hdr = ReadExact(from, 4);
                if (hdr == null) break;
                int len = (hdr[2] << 8) | hdr[3];
                if (len < 4 || len > 8192) break;
                byte[] rest = ReadExact(from, len - 4);
                if (rest == null) break;
                byte[] pkt = new byte[len];
                Buffer.BlockCopy(hdr, 0, pkt, 0, 4);
                Buffer.BlockCopy(rest, 0, pkt, 4, len - 4);
                int n = Interlocked.Increment(ref seq.n);
                File.WriteAllBytes(Path.Combine(outDir, $"{tag}-{n:D2}.bin"), pkt);
                Console.WriteLine($"{tag}-{n:D2} len={len} head={BitConverter.ToString(pkt, 0, Math.Min(len, 24))}");
                to.Write(pkt, 0, pkt.Length);
            }
        }
        catch { /* 对端关闭即停泵 */ }
    }

    static byte[] ReadExact(NetworkStream s, int n)
    {
        byte[] buf = new byte[n];
        int off = 0;
        while (off < n)
        {
            int r = s.Read(buf, off, n - off);
            if (r <= 0) return null;
            off += r;
        }
        return buf;
    }

    // 接受第一个携带数据的连接（跳过 TCPPing 等无数据即关的探测连接）。
    static TcpClient AcceptDataConnection(TcpListener tap, int timeoutMs)
    {
        var sw = System.Diagnostics.Stopwatch.StartNew();
        while (sw.ElapsedMilliseconds < timeoutMs)
        {
            if (tap.Pending())
            {
                var c = tap.AcceptTcpClient();
                // 可读（含对端已关）或 2s 内到达数据 → 判定
                if (c.Client.Poll(2000000, SelectMode.SelectRead))
                {
                    if (c.Available > 0) return c;
                    try { c.Close(); } catch { } // 无数据的半关：ping，丢弃
                }
                else
                {
                    // 2s 无动静：可能是尚未发包的真连接，先留后判
                    // （ping 一定会关，真连接一定会发 CR；再给一次机会）
                    if (c.Client.Poll(2000000, SelectMode.SelectRead) && c.Available > 0) return c;
                    try { c.Close(); } catch { }
                }
            }
            else Thread.Sleep(50);
        }
        throw new Exception("accept timeout：无携带数据的连接");
    }

    static IPEndPoint ParseEp(string s)
    {
        var p = s.Split(':');
        return new IPEndPoint(IPAddress.Parse(p[0]), int.Parse(p[1]));
    }

    static Args Parse(string[] argv)
    {
        var a = new Args();
        for (int i = 0; i < argv.Length; i++)
        {
            string k = argv[i];
            string v = (i + 1 < argv.Length) ? argv[++i] : "";
            switch (k)
            {
                case "--emulator": a.Emulator = v; break;
                case "--out": a.Out = v; break;
                case "--unit": a.Unit = int.Parse(v); break;
                case "--module": a.Module = int.Parse(v); break;
                case "--param": a.Param = int.Parse(v); break;
                case "--start": a.Start = int.Parse(v); break;
                case "--amount": a.Amount = int.Parse(v); break;
                case "--wordlen": a.WordLen = int.Parse(v); break;
                default: throw new Exception($"未知参数 {k}");
            }
        }
        return a;
    }

    static int Fail(string msg, int rc)
    {
        Console.WriteLine($"FAIL {msg} rc={rc}");
        return 1;
    }
}
