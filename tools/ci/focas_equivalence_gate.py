"""当前 FOCAS 17类声明输出的软件验收入口；设备兼容性和发布许可仍单独验收。

必须重新构建，运行单测/固定报文实际 DLL 差分/正式合同入口；不接受跳过参数。
只连接本机临时服务；任何失败均保存失败结论，不把旧结果当本次通过证据。
"""
import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
DLL_HASHES = {
    'fwlib64.dll': '7b3837e3925902d1e06c5c7e4f9ce39fe7f39aa9caed029096187f5641ec04ab',
    'fwlibe64.dll': 'd3341b43bf3945bdb49d03d1d96436a75911bf3c8d1cdf4f30b9167ef935bfd6',
}
READY = {
    'status':'machine/status', 'feed':'machine/feed', 'speed':'machine/spindle_speed',
    'absolute':'axis/absolute', 'gear':'spindle/gear', 'maxrpm':'spindle/maxrpm',
    'macro':'macro/value', 'pmc-byte':'pmc/value', 'pmc-word':'pmc/value',
    'pmc-dword':'pmc/value', 'pmc-bit':'pmc/value', 'param':'param/value',
    'diagnosis':'diagnosis/value', 'opmsg':'opmsg/value', 'alarm':'alarm/value',
    'offset':'tool/offset', 'length':'tool/length', 'zofs':'tool/zofs',
}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read(path):
    return json.loads(path.read_text(encoding='utf-8'))


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding='utf-8')


def source_hash():
    # HEAD 无法标识未提交和未跟踪代码；同时冻结实际源文件集合与内容。
    paths = subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard'], cwd=ROOT)
    digest = hashlib.sha256()
    for name in sorted(set(paths.split(b'\0')) - {b''}):
        path = ROOT/os.fsdecode(name)
        digest.update(name+b'\0')
        digest.update(hashlib.sha256(path.read_bytes()).digest() if path.is_file() else b'missing')
    return digest.hexdigest()


def check_modules(modules):
    actual = {pathlib.Path(m['path']).name.lower():m['sha256'] for m in modules}
    if any(actual.get(name) != value for name,value in DLL_HASHES.items()):
        raise ValueError('实际加载 DLL 不等于本次冻结版本；必须重新建立版本对照')


def run(out, name, command, timeout=600, cwd=ROOT):
    print(f'[{name}] 开始', flush=True)
    with (out/f'{name}.log').open('wb') as log:
        result = subprocess.run(command, cwd=cwd, stdout=log, stderr=subprocess.STDOUT, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f'{name} 失败（exit={result.returncode}），详见对应日志')
    print(f'[{name}] 通过', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    args = parser.parse_args()
    if os.name != 'nt':
        parser.error('实际参考 DLL 差分入口要求 Windows；跨平台 CI 使用归档回放测试')
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    before = source_hash()
    result = {'scope':'当前17类声明读取输出的软件基线，非DLL全部导出函数',
              'generated_at_ns':time.time_ns(), 'source_sha256':before,
              'git_sha':subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT, text=True).strip(),
              'software_gate_passed':False, 'production_release_ready':False,
              'hardware_acceptance':False,
              'remaining_gates':['实际 FANUC 硬件验收','其他固件/平台兼容性与 SDK 许可发布确认'],
              'support_boundary':{'ready':sorted(set(READY.values()) | {'spindle/load','servo/load'}), 'hold':[]},
              'load_contract':{'type':'F64','unit':'%','indices':'1..4','missing':'BAD',
                               'conversion':'DLL raw + signed decimal; finite F64, no clamp',
                               'protocol_specs':[1,2,3,4]}}
    try:
        run(out,'fmt',['cargo','fmt','--all','--','--check'])
        run(out,'clippy',['cargo','clippy','--locked','-p','mesa-driver-focas2','--all-targets','--','-D','warnings'])
        run(out,'build',['cargo','test','--locked','-p','mesa-driver-focas2','--lib','--no-run','--message-format=json'])
        artifacts = [json.loads(line) for line in (out/'build.log').read_text(encoding='utf-8',errors='replace').splitlines() if line.startswith('{')]
        binaries = [r['executable'] for r in artifacts if r.get('reason') == 'compiler-artifact'
                    and r['profile']['test'] and r['target']['name'] == 'mesa_driver_focas2']
        if len(binaries) != 1:
            raise ValueError('无法唯一确认最新测试程序')
        exe = binaries[0]
        result['test_exe_sha256'] = sha(pathlib.Path(exe).read_bytes())
        run(out,'unit',[exe],timeout=120,cwd=ROOT/'drivers/focas2')
        summary = re.search(r'test result: ok\. (\d+) passed; 0 failed; (\d+) ignored',
                            (out/'unit.log').read_text(encoding='utf-8',errors='replace'))
        if summary is None or int(summary[1]) == 0:
            raise ValueError('缺少单测真实通过汇总')
        result['unit'] = {'passed':int(summary[1]),'ignored':int(summary[2])}
        run(out,'shadow-unit',['cargo','test','--locked','-p','mesa-driver-focas2','--example','shadow_probe'])
        run(out,'frame-tools',[sys.executable,'-B','tools/ci/test_focas_diff.py'])
        profiles = []
        for profile in ('baseline','minimum','maximum'):
            run(out,'dll-'+profile,[sys.executable,'-B','tools/ci/focas_ready_synthetic.py',
                '--test-exe',exe,'--profile',profile,'--out',str(out/profile)],timeout=120)
            data = read(out/profile/'manifest.json')
            check_modules(data['loaded_modules'])
            for file,hash_value in data['files'].items():
                if sha((out/profile/file).read_bytes()) != hash_value:
                    raise ValueError('DLL 差分证据文件哈希不符')
            rows = read(out/profile/'samples/comparison.json')
            if {r['name'] for r in rows} != set(READY) or len(rows) != len(READY) or not all(r['equal'] for r in rows):
                raise ValueError('READY 范围遗漏或存在不一致，不能豁免 Unsupported')
            profiles.append({'profile':profile,'cases':len(rows),'equal':True})
        result['dll_ready'] = {'profiles':profiles,'cases':sum(p['cases'] for p in profiles),'categories':15}
        run(out,'dll-errors',[sys.executable,'-B','tools/ci/focas_opmsg_synthetic.py',
            '--test-exe',exe,'--out',str(out/'errors')],timeout=120)
        check_modules(read(out/'errors/samples/modules.json'))
        error_data = read(out/'errors/manifest.json')
        if not error_data['all_equal']:
            raise ValueError('消息/返回码差分未通过')
        result['opmsg_and_return_codes'] = {'cases':len(error_data['cases']),'equal':True,
                                             'limit':'错误报文对照不等于连接故障/超时全等证明'}
        run(out,'dll-load-research',[sys.executable,'-B','tools/ci/focas_load_synthetic.py',
            '--test-exe',exe,'--out',str(out/'load-research')],timeout=120)
        load_data = read(out/'load-research/index.json')
        for case in load_data['cases']:
            modules = {m['name'].lower():m['sha256'] for m in case['runtime_modules']}
            if any(modules.get(k) != v for k,v in DLL_HASHES.items()):
                raise ValueError('负载研究 DLL 身份不符')
        result['load_research'] = {'cases':len(load_data['cases']),'operations':4*len(load_data['cases']),
                                  'production_accepted':False}
        run(out,'dll-load-production',[sys.executable,'-B','tools/ci/focas_load_production.py',
            '--test-exe',exe,'--out',str(out/'load-production')],timeout=180)
        load_production = read(out/'load-production/index.json')
        required_cases = {'zero','fractional','four','decimal10','decimal_extremes','shrinking','empty',
                          'legacy_direct','legacy_scaled','legacy_global_axes','legacy_wrapping'}
        if {c['name'] for c in load_production['cases']} != required_cases or not load_production['all_equal']:
            raise ValueError('正式负载差分缺失分支/边界或不一致')
        for case in load_production['cases']:
            check_modules(case['loaded_modules'])
            if not case['all_equal'] or case['rows'] != 24:
                raise ValueError('正式负载差分行数或结果异常')
            for filename, expected_hash in case['files'].items():
                if sha((out/'load-production'/case['name']/filename).read_bytes()) != expected_hash:
                    raise ValueError('正式负载差分证据文件哈希不符')
        result['load_production'] = {'cases':len(required_cases),'comparisons':264,'categories':2,
                                    'equal':True,'batch_dedup':True,'legacy_status4_cache':True,
                                    'engineering_tolerance':'relative 1e-14 + one smallest F64 subnormal',
                                    'hardware_evidence':False}
        # 唯一可信合同入口，不接收外部旧 JSON，不绕过独立 Driver bin 重编。
        run(out,'contract',[sys.executable,'-B','scripts/write-contract-evidence.py'],timeout=900)
        contract_path = ROOT/'target/validation/contract.json'
        contract = read(contract_path)
        if contract['failed'] or contract['test_counts']['failed_tests'] or not contract['test_counts']['passed_tests']:
            raise ValueError('正式合同证据未通过')
        (out/'contract.json').write_bytes(contract_path.read_bytes())
        result['contract'] = {'suites':contract['total'],'tests':contract['test_counts']['passed_tests'],
                              'dirty':contract['dirty'],'sha256':sha(contract_path.read_bytes())}
        if source_hash() != before:
            raise ValueError('验收期间源文件发生变化，结果不能绑定同一版本')
        result['software_gate_passed'] = True
    except Exception as error:
        result['failure'] = str(error)
        write(out/'equivalence.json',result)
        raise
    write(out/'equivalence.json',result)
    print(json.dumps(result, ensure_ascii=False))


if __name__ == '__main__':
    main()
