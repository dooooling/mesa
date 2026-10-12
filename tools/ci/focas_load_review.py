"""按测试 PID 的 TCP 四元组核对负载采集，--archive 可归档新 fixture。

输入 run-dir 内的 samples 与 flows/report.json；任何缺口、调用缺失、请求差异或
Native 缓冲检查失败均拒绝归档。不把零负载提升为生产准入或非零负载证据。
"""
import argparse
import hashlib
import json
import pathlib
import struct

from focas_diff import read_valid_request


def sha(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    return json.loads(path.read_text(encoding='utf-8'))


def replies(frame):
    if frame[:4] != b'\xa0'*4 or frame[6:8] != b'\x21\x02':
        raise ValueError('响应帧类型非法')
    if len(frame) != 10+struct.unpack_from('>H', frame, 8)[0]:
        raise ValueError('响应帧长度不闭合')
    data, offset, slots = frame[10:], 2, []
    for _ in range(struct.unpack_from('>H', data)[0]):
        size,dev,path,cmd,status,d1,d2,length = struct.unpack_from('>4H3hH',data,offset)
        if size != 16+length or offset+size > len(data) or status != 0:
            raise ValueError('响应子包长度非法或状态非零')
        slots.append((dev,path,cmd,data[offset+16:offset+size]))
        offset += size
    if offset != len(data):
        raise ValueError('响应尾部未耗尽')
    return slots


def native_from_reply(response, snapshot, tag):
    slots = replies(response)
    commands = [0xa4,0x89,0x56,0xa4] if tag == 'servo' else [0xa4,0x8a]+[0x40]*(2 if tag == 'type_all' else 1)+[0xa4]
    if [(s[0],s[1],s[2]) for s in slots] != [(1,1,c) for c in commands]:
        raise ValueError('响应槽身份与操作不一致')
    before, after = [struct.unpack('>H',slots[i][3])[0] for i in (0,-1)]
    count = min(snapshot['num_in'],before,after)
    if count != snapshot['num_out'] or len(slots[1][3]) < count*4:
        raise ValueError('Native 数量与响应数量不一致')
    expected = []
    sides = [0] if tag in ('type0','servo') else [1] if tag == 'type1' else [0,1]
    for record in range(count):
        name = list(slots[1][3][record*4:record*4+3])
        for value_slot,side in enumerate(sides,2):
            raw,aux,decimal = struct.unpack_from('>iHh',slots[value_slot][3],record*8)
            expected.append({'slot':record if tag == 'servo' else record*2+side,
                             'data':abs(raw) if tag == 'servo' else raw,
                             'decimal':decimal,'unit':side,'name':name})
    # ABI 输出为按 record 排序；检查来自对应 Native 响应，而非另一条 Wire 响应。
    if expected != snapshot['candidates']:
        raise ValueError('Native 响应与 ABI 输出不一致')
    return {'before':before,'after':after,'num_out':count}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run',type=pathlib.Path)
    parser.add_argument('--archive',type=pathlib.Path)
    args = parser.parse_args()
    run = args.run.resolve(strict=True)
    session = load(run/'samples/session.json')
    report = load(run/'flows/report.json')
    capture = load(run/'capture.json')
    if capture['test_exit_code'] != 0 or capture['overflow']:
        raise ValueError('采样失败或捕获超过上限')

    def own_flows(backend):
        tuples = load(run/f'samples/{backend}-connections.json')
        if any(row['pid'] != session['pid'] for row in tuples):
            raise ValueError('TCP PID 与测试进程不一致')
        matching = [f for f in report['flows'] if f['connection'] in [r['connection'] for r in tuples]]
        if len(matching) != len(tuples) or not matching:
            raise ValueError('TCP 表连接未在抓包中完整找到')
        pairs = []
        for flow in matching:
            dirs = flow['directions']
            if any(m.get('reassembly_errors') or m.get('frame_error') for m in dirs.values()):
                raise ValueError('所属 TCP 流重组或拆帧失败')
            for d,open_type,close_type in [('c2s','0101','0201'),('s2c','0102','0202')]:
                frames = dirs[d].get('frames',[])
                if not frames or frames[0]['type'] != open_type or frames[-1]['type'] != close_type:
                    raise ValueError('所属流缺少 OPEN 或 CLOSE')
            requests = [x for x in dirs['c2s']['frames'] if x['type']=='2101']
            responses = [x for x in dirs['s2c']['frames'] if x['type']=='2102']
            if len(requests) != len(responses):
                raise ValueError('请求响应数量不一致')
            for request,response in zip(requests,responses):
                # Native 建连另有系统信息查询，单独识别，不当成负载调用。
                if [s['func'] for s in request['request']['subs']] == ['0x00010018']:
                    continue
                pairs.append((request,response,flow['connection']))
        if len(pairs) != 4:
            raise ValueError(f'{backend} 负载调用必须恰好四次')
        return pairs

    native,wire = own_flows('native'),own_flows('wire')
    records,files = [],{}
    for tag,np,wp in zip(('type0','type1','type_all','servo'),native,wire):
        request = read_valid_request(str(run/f'samples/wire-{tag}.request.bin'))
        if request != pathlib.Path(np[0]['file']).read_bytes() or request != pathlib.Path(wp[0]['file']).read_bytes():
            raise ValueError('Native/Wire 捕获请求不等于正确请求构造')
        nr,wr = pathlib.Path(np[1]['file']).read_bytes(),pathlib.Path(wp[1]['file']).read_bytes()
        if wr != (run/f'samples/wire-{tag}.response.bin').read_bytes():
            raise ValueError('Wire 捕获响应与程序收到的响应不同')
        snapshot = load(run/f'samples/native-{tag}.json')
        if snapshot['rc'] != 0 or not all(snapshot[k] for k in ('guards_ok','tail_ok','span_tail_ok','untouched_half_ok')):
            raise ValueError('Native 状态或缓冲检查不通过')
        counts = native_from_reply(nr,snapshot,tag)
        comparison = load(run/f'samples/wire-{tag}.json')
        if not comparison['native_candidates_equal'] or snapshot['candidates'] != comparison['candidates']:
            raise ValueError('顺序采样值不同；必须保留现场调查，不归档为一致样本')
        binary = {'native_request.bin':request,'native_response.bin':nr,'wire_request.bin':request,'wire_response.bin':wr}
        files[tag] = binary
        records.append({'tag':tag,'num_in':snapshot['num_in'],**counts,
                        'native_connection':np[2],'wire_connection':wp[2],
                        'request_bytes_equal':True,'native_candidates':snapshot['candidates'],
                        'wire_candidates':comparison['candidates'],
                        'sha256':{name:sha(data) for name,data in binary.items()}})
    modules = load(run/'samples/modules-type0.json')
    result = {'capture_sha256':capture['capture_sha256'],'git_sha':capture['git_sha'],
              'test_exe_sha256':capture['test_exe_sha256'],'pid':session['pid'],
              'scope':'NCGuide 零负载同轮顺序采样；非原子快照，非生产准入',
              'started_unix_ns':capture['started_unix_ns'],
              'runtime_modules':[{'name':pathlib.Path(m['path']).name,'sha256':m['sha256']} for m in modules],
              'records':records,'production_gate_changed':False,
              'limits':['仅当前 DLL/NCGuide 路径','无非零负载、多主轴、旧缩放回退证据','Npcap 内核丢包数未取得']}
    output = args.archive.resolve() if args.archive else run/'review'
    output.mkdir(parents=True,exist_ok=False)
    for record in records:
        folder = output/record['tag']
        folder.mkdir()
        for name,data in files[record['tag']].items():
            (folder/name).write_bytes(data)
        (folder/'expected.json').write_text(json.dumps(record,ensure_ascii=False,indent=2),encoding='utf-8')
    (output/'index.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps({'archive':str(output),'verified_pairs':4,'scope':result['scope']},ensure_ascii=False))


if __name__ == '__main__':
    main()
