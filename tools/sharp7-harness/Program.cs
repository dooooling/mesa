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
        public int WordLen = 0x1A; // S7WLDouble（8 bytes；与 emulator element-size 8 等价，F3 证据等价要求）
        public string ExpectSingle = ""; // hex（可带-）：单读 exact 内容期望，不提供则只验长度
        public string ExpectMulti0 = ""; // multi 首项 exact 内容期望
        public string ExpectMulti1 = ""; // multi 次项 exact 内容期望
    }

    // 期望解码字节数 = Amount × WordSize（WordLen 决定；DOUBLE=8）。
    // F3 要求 Sharp7 与 Mesa 对同一 wire 得到相同语义数据（exact bytes），
    // 不止 framing/status 一致。
    static int ExpectedBytes(Args a) => a.Amount * WordSize(a.WordLen);

    static int WordSize(int wordLen) => wordLen switch
    {
        0x1A => 8, // S7WLDouble
        0x02 => 1, // S7WLByte
        _ => -1,   // 未支持：拒绝猜
    };

    // hex 解析（BitConverter 风格，可带 '-' 分隔；非法即抛，由调用方判失败）。
    static byte[] ParseHex(string h)
    {
        string s = h.Replace("-", "");
        if (s.Length == 0 || s.Length % 2 != 0) throw new Exception($"hex 非法 {h}");
        var b = new byte[s.Length / 2];
        for (int i = 0; i < b.Length; i++) b[i] = Convert.ToByte(s.Substring(i * 2, 2), 16);
        return b;
    }

    // exact 内容比对（长度 + 逐字节）；不一致打印首个差异位并返回 false。
    static bool ExactIs(byte[] got, int count, byte[] want, string tag)
    {
        if (count != want.Length)
        {
            Console.WriteLine($"{tag} 长度 {count} != 期望 {want.Length}");
            return false;
        }
        for (int i = 0; i < want.Length; i++)
            if (got[i] != want[i])
            {
                Console.WriteLine($"{tag}[{i}] {got[i]:X2} != 期望 {want[i]:X2}");
                return false;
            }
        return true;
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
        // 严格 exit 码（证据工具禁 false-green）：任何一步失败即非零退出；
        // clientDone 必在 finally 置位，避免主线程空等。
        var clientDone = new ManualResetEventSlim(false);
        int clientRc = 0;
        var clientThread = new Thread(() =>
        {
            var client = new S7Client();
            try
            {
                int expect = ExpectedBytes(a);
                if (expect <= 0 || expect > 256) { Console.WriteLine($"bad wordlen {a.WordLen}"); clientRc = 10; return; }
                client.PLCPort = tapPort;
                int rc = client.NckConnectTo("127.0.0.1");
                Console.WriteLine($"NckConnectTo rc={rc}");
                if (rc != 0) { clientRc = 11; return; }

                // --- single read（ReadNckArea）：要求 exact bytes ---
                byte[] singleBuf = new byte[256];
                int bytesRead = 0;
                rc = client.ReadNckArea(0, a.Unit, a.Module, a.Param, a.Start, a.Amount, a.WordLen, singleBuf, ref bytesRead);
                Console.WriteLine($"ReadNckArea rc={rc} bytesRead={bytesRead} data={BitConverter.ToString(singleBuf, 0, Math.Max(bytesRead, 0))}");
                lock (events) { events.Add(new { op = "single", rc, bytesRead, data = BitConverter.ToString(singleBuf, 0, Math.Max(bytesRead, 0)) }); }
                if (rc != 0) { clientRc = 12; return; }
                if (bytesRead != expect) { Console.WriteLine($"single bytes mismatch {bytesRead} != {expect}"); clientRc = 13; return; }
                // exact 内容 gating（提供 --expect-single 时）：长度对了还不够，
                // 内容必须逐字节一致，否则证据无效。
                if (a.ExpectSingle.Length > 0 && !ExactIs(singleBuf, bytesRead, ParseHex(a.ExpectSingle), "single"))
                { clientRc = 16; return; }

                // --- multi read（ReadMultiNckVars，两项：param 与 param+1） ---
                var multi = new S7NckMultiVar(client);
                byte[] buf0 = new byte[256];
                byte[] buf1 = new byte[256];
                multi.NckAdd(0, a.Unit, a.Module, a.Param, a.WordLen, a.Start, a.Amount, ref buf0);
                multi.NckAdd(0, a.Unit, a.Module, a.Param + 1, a.WordLen, a.Start, a.Amount, ref buf1);
                rc = multi.ReadNck();
                Console.WriteLine($"MultiRead rc={rc} results=[{multi.Results[0]},{multi.Results[1]}] buf0={BitConverter.ToString(buf0, 0, expect)} buf1={BitConverter.ToString(buf1, 0, expect)}");
                lock (events) { events.Add(new { op = "multi", rc, results = new[] { multi.Results[0], multi.Results[1] }, buf0 = BitConverter.ToString(buf0, 0, expect), buf1 = BitConverter.ToString(buf1, 0, expect) }); }
                if (rc != 0) { clientRc = 14; return; }
                if (multi.Results[0] != 0 || multi.Results[1] != 0) { Console.WriteLine("multi item BAD"); clientRc = 15; return; }
                if (a.ExpectMulti0.Length > 0 && !ExactIs(buf0, expect, ParseHex(a.ExpectMulti0), "multi0"))
                { clientRc = 17; return; }
                if (a.ExpectMulti1.Length > 0 && !ExactIs(buf1, expect, ParseHex(a.ExpectMulti1), "multi1"))
                { clientRc = 18; return; }

                client.Disconnect();
            }
            catch (Exception e) { Console.WriteLine($"client end: {e.Message}"); clientRc = 1; }
            finally
            {
                try { client.Disconnect(); } catch { }
                clientDone.Set();
            }
        });
        clientThread.IsBackground = true;
        clientThread.Start();

        if (!acceptDone.Wait(10000)) return Fail("tap accept timeout", -1);
        // tap 双向泵跑在独立线程（req = client→server，rsp = 反向）。
        // accept 已发生，client 握手字节已在内核缓冲，pump 接管即转发，不丢包。
        var pumpSt = new PumpState();
        var pumpThread = new Thread(() =>
        {
            try { Pump(fromClient.GetStream(), toEmu.GetStream(), a.Out, seq, pumpSt); }
            catch (Exception e) { Console.WriteLine($"pump end: {e.Message}"); Interlocked.CompareExchange(ref pumpSt.rc, 21, 0); }
        });
        pumpThread.IsBackground = true;
        pumpThread.Start();
        if (!clientDone.Wait(60000)) { Console.WriteLine("client timeout"); clientRc = 31; }
        // 确定性 teardown：先立预期关闭旗，再关两端 socket 解开阻塞读。
        teardown = true;
        try { fromClient.Close(); } catch { }
        try { toEmu.Close(); } catch { }
        // Join 超时即失败（pump 线程泄漏/死锁不得静默）。
        if (!pumpThread.Join(5000)) { Console.WriteLine("pump join timeout"); return 32; }
        tap.Stop();
        // client 优先（更有信息量），pump 错误其次。
        if (clientRc != 0) return clientRc;
        if (pumpSt.rc != 0) return pumpSt.rc;

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
    // teardown 预期关闭旗：teardown 后 socket 读写抛异常属预期，不记错。
    static volatile bool teardown;

    // 一次读一整包 TPKT（4 字节头给长度），双向各自编号落盘。
    // worker 异常经 pumpRc 上报（set-once，首错为准），不再 catch-all 静默。
    sealed class PumpState { public int rc; }
    static void Pump(NetworkStream c2s, NetworkStream s2c, string outDir, Seq seq, PumpState st)
    {
        var t1 = new Thread(() => Forward(c2s, s2c, outDir, "req", seq, st));
        var t2 = new Thread(() => Forward(s2c, c2s, outDir, "rsp", seq, st));
        t1.IsBackground = t2.IsBackground = true;
        t1.Start(); t2.Start();
        t1.Join(); t2.Join();
    }

    static void Forward(NetworkStream from, NetworkStream to, string outDir, string tag, Seq seq, PumpState st)
    {
        try
        {
            while (true)
            {
                byte[] hdr = ReadExact(from, 4);
                if (hdr == null) break; // 对端正常关：EOF 即停泵（非错）
                int len = (hdr[2] << 8) | hdr[3];
                if (len < 4 || len > 8192)
                {
                    if (!teardown) { Console.WriteLine($"{tag} 非法 TPKT 长 {len}"); Interlocked.CompareExchange(ref st.rc, 22, 0); }
                    break;
                }
                byte[] rest = ReadExact(from, len - 4);
                if (rest == null) break; // 同上：包内 EOF 即停泵
                byte[] pkt = new byte[len];
                Buffer.BlockCopy(hdr, 0, pkt, 0, 4);
                Buffer.BlockCopy(rest, 0, pkt, 4, len - 4);
                int n = Interlocked.Increment(ref seq.n);
                File.WriteAllBytes(Path.Combine(outDir, $"{tag}-{n:D2}.bin"), pkt);
                Console.WriteLine($"{tag}-{n:D2} len={len} head={BitConverter.ToString(pkt, 0, Math.Min(len, 24))}");
                to.Write(pkt, 0, pkt.Length);
            }
        }
        catch (Exception e)
        {
            // teardown 预期关闭（我们主动关 socket 解阻塞读）不记错；
            // 其余一律上报（首错为准）。
            if (!teardown)
            {
                Console.WriteLine($"{tag} pump 异常: {e.GetType().Name}: {e.Message}");
                Interlocked.CompareExchange(ref st.rc, 22, 0);
            }
        }
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
                case "--expect-single": a.ExpectSingle = v; break;
                case "--expect-multi0": a.ExpectMulti0 = v; break;
                case "--expect-multi1": a.ExpectMulti1 = v; break;
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
