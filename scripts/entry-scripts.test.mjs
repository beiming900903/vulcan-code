// Fill the entry script templates with synthetic setup programs and run the real scripts against them.
// 以合成的安装器填写入口脚本模板，并针对它们运行真实脚本。
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { copyFile, chmod, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { createReadStream } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { pathToFileURL } from 'node:url';
import { releasePlatforms } from './desktop-runtime.mjs';
import { entryScripts, generateEntryScripts, renderTemplate, setupProgramName } from './entry-scripts.mjs';

const version = '1.2.3';

// withRoot runs action with a temporary directory that is removed afterwards.
// withRoot 以一个临时目录运行 action，结束后删除该目录。
async function withRoot(action) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'vulcan-entry-'));
  try {
    return await action(root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

// writeSetups lays out an origin holding one setup program per platform; program(platform) gives its bytes.
// writeSetups 布置一个下载源，每个平台一个安装器；program(platform) 给出其字节。
async function writeSetups(origin, program) {
  const dir = path.join(origin, 'setup', version);
  await mkdir(dir, { recursive: true });
  for (const platform of releasePlatforms) {
    const file = path.join(dir, setupProgramName(version, platform));
    await writeFile(file, await program(platform));
    await chmod(file, 0o755);
  }
  return dir;
}

test('templates and values must match exactly', () => {
  assert.equal(renderTemplate('a @@X@@ b', { X: '1' }), 'a 1 b');
  assert.throws(() => renderTemplate('a @@X@@ @@Y@@', { X: '1' }), /value for Y/);
  assert.throws(() => renderTemplate('a @@X@@', { X: '1', Z: '2' }), /no place for Z/);
  assert.throws(() => renderTemplate('a @@x@@', {}), /malformed/);
  assert.throws(() => setupProgramName(version, 'windows-arm64'), /Unknown release platform/);
  assert.equal(setupProgramName(version, 'windows-x64'), `vulcan-code-setup-${version}-windows-x64.exe`);
});

test('generation needs every platform, a valid signer and plain origins', () => withRoot(async root => {
  const setups = await writeSetups(root, async platform => platform);
  const out = path.join(root, 'out');
  const base = { version, setups, origins: ['https://primary.example.test/vc', 'https://china.example.test/vc'], signer: null, out };
  const written = await generateEntryScripts(base);
  assert.deepEqual(written.map(file => path.basename(file)), [...entryScripts]);
  for (const file of written) assert.doesNotMatch(await readFile(file, 'utf8'), /@@/);
  await assert.rejects(generateEntryScripts({ ...base, signer: 'not-a-thumbprint' }), /thumbprint/);
  await assert.rejects(generateEntryScripts({ ...base, origins: [base.origins[0], "https://x.test/'"] }), /Invalid download origin/);
  await assert.rejects(generateEntryScripts({ ...base, origins: [base.origins[0], 'https://x.test/'] }), /Invalid download origin/);
  await assert.rejects(generateEntryScripts({ ...base, origins: [base.origins[0]] }), /two distinct download origins/);
  await rm(path.join(setups, setupProgramName(version, 'macos-arm64')));
  await assert.rejects(generateEntryScripts(base), /ENOENT/);
}));

// install.sh is a POSIX shell script for Linux and macOS.
// install.sh 是面向 Linux 与 macOS 的 POSIX shell 脚本。
const noShell = process.platform === 'win32' || spawnSync('curl', ['--version']).status !== 0;

test('install.sh picks the platform, falls back between origins, verifies and passes everything on', { skip: noShell }, () => withRoot(async root => {
  const origin = path.join(root, 'origin');
  // Each fake setup program reports its platform, its arguments and whether input is a terminal.
  // 每个伪造的安装器报告其平台、参数以及输入是否为终端。
  const setups = await writeSetups(origin, async platform => `#!/bin/sh\necho "${platform} $*"\nexit 7\n`);
  const out = path.join(root, 'out');
  const missing = pathToFileURL(path.join(root, 'missing')).href;
  await generateEntryScripts({ version, setups, origins: [missing, pathToFileURL(origin).href], signer: null, out });
  const fakes = path.join(root, 'fakes');
  await mkdir(fakes);
  await writeFile(path.join(fakes, 'uname'), '#!/bin/sh\ncase "$1" in -s) echo "$FAKE_OS" ;; -m) echo "$FAKE_MACHINE" ;; *) exit 64 ;; esac\n');
  await writeFile(path.join(fakes, 'sysctl'), '#!/bin/sh\necho "$FAKE_TRANSLATED"\n');
  await chmod(path.join(fakes, 'uname'), 0o755);
  await chmod(path.join(fakes, 'sysctl'), 0o755);
  const run = (env, args = ['--mode', 'terminal', '--yes']) => spawnSync('sh', [path.join(out, 'install.sh'), ...args], {
    encoding: 'utf8',
    input: '',
    env: { ...process.env, FAKE_TRANSLATED: '0', HOME: root, XDG_DATA_HOME: '', ...env, PATH: `${fakes}:${process.env.PATH}`, TMPDIR: root },
  });

  const linux = run({ FAKE_OS: 'Linux', FAKE_MACHINE: 'aarch64' });
  assert.equal(linux.status, 7, linux.stderr);
  assert.equal(linux.stdout.trim(), 'linux-arm64 --mode terminal --yes');
  assert.match(linux.stderr, /trying the next origin/);
  // Selecting China skips an unreachable primary; selecting international does not fall back.
  // 选择中国大陆节点时跳过不可达的主源；选择国际节点时不回退。
  const china = run({ FAKE_OS: 'Linux', FAKE_MACHINE: 'aarch64' }, ['--route', 'china', '--yes']);
  assert.equal(china.status, 7, china.stderr);
  assert.match(china.stdout, /--route china/);
  assert.doesNotMatch(china.stderr, /\/missing\/|trying the next origin/);
  const international = run({ FAKE_OS: 'Linux', FAKE_MACHINE: 'aarch64' }, ['--route=international']);
  assert.equal(international.status, 1, international.stderr);
  assert.doesNotMatch(international.stdout, /linux-arm64/);
  assert.doesNotMatch(international.stderr, /trying the next origin/);
  const rosetta = run({ FAKE_OS: 'Darwin', FAKE_MACHINE: 'x86_64', FAKE_TRANSLATED: '1' }, []);
  assert.equal(rosetta.stdout.trim(), 'macos-arm64');
  assert.equal(run({ FAKE_OS: 'Darwin', FAKE_MACHINE: 'x86_64' }, []).stdout.trim(), 'macos-x64');
  assert.match(run({ FAKE_OS: 'Linux', FAKE_MACHINE: 'riscv64' }).stderr, /unsupported architecture/);
  assert.match(run({ FAKE_OS: 'FreeBSD', FAKE_MACHINE: 'x86_64' }).stderr, /unsupported operating system/);

  // A program that differs from its published digest never runs.
  // 与公布摘要不符的程序绝不运行。
  await writeFile(path.join(setups, setupProgramName(version, 'linux-x64')), '#!/bin/sh\necho tampered\n');
  const tampered = run({ FAKE_OS: 'Linux', FAKE_MACHINE: 'x86_64' });
  assert.equal(tampered.status, 1);
  assert.doesNotMatch(tampered.stdout, /tampered/);
  assert.match(tampered.stderr, /does not match its published SHA-256/);

  const installedRoot = path.join(root, 'custom install');
  const setupDir = path.join(installedRoot, 'setup');
  const program = path.join(setupDir, 'vulcan-code-setup');
  await mkdir(setupDir, { recursive: true });
  await writeFile(path.join(setupDir, 'installation.json'), '{}');
  await writeFile(program, '#!/bin/sh\nprintf "<%s>\\n" "$@"\nexit 7\n', { mode: 0o755 });
  const env = { FAKE_OS: 'Linux', FAKE_MACHINE: 'aarch64' };
  const reused = run(env, ['--prefix', installedRoot, '--yes']);
  assert.equal(reused.status, 7, reused.stderr);
  assert.deepEqual(reused.stdout.trim().split('\n'), ['<--prefix>', `<${installedRoot}>`, '<--yes>', '<--self-update>']);
  assert.doesNotMatch(reused.stderr, /Downloading/);
  const optedOut = run(env, ['update', `--prefix=${installedRoot}`, '--no-self-update']);
  assert.doesNotMatch(optedOut.stdout, /<--self-update>/);
  assert.doesNotMatch(run(env, ['status', '--prefix', installedRoot]).stdout, /<--self-update>/);

  const data = path.join(root, 'xdg');
  await mkdir(data);
  await rename(installedRoot, path.join(data, 'vulcan-code'));
  const fromXdg = run({ ...env, XDG_DATA_HOME: data }, []);
  assert.equal(fromXdg.status, 7, fromXdg.stderr);
  assert.equal(fromXdg.stdout.trim(), '<--self-update>');
}));

// The Windows script is checked in every PowerShell this machine has; 5.1 is the one that matters most.
// Windows 脚本在本机已有的每种 PowerShell 中检查；5.1 最为关键。
const powerShells = process.platform === 'win32'
  ? ['powershell.exe', 'pwsh.exe'].filter(shell => spawnSync(shell, ['-NoProfile', '-Command', 'exit 0']).status === 0)
  : [];

// runAsync runs a command without blocking the event loop, so the local server can answer it.
// runAsync 运行命令而不阻塞事件循环，使本地服务器能够应答。
function runAsync(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { windowsHide: true });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    child.on('error', reject);
    child.on('close', status => resolve({ status, stdout, stderr }));
  });
}

test('install.ps1 verifies the setup program and passes its arguments and exit code on', { skip: powerShells.length === 0 }, () => withRoot(async root => {
  const origin = path.join(root, 'origin');
  // cmd.exe stands in for the setup program: `/d /c` runs whatever the test passes.
  // cmd.exe 代替安装器：`/d /c` 运行测试传入的任何内容。
  const cmd = path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'cmd.exe');
  const setups = await writeSetups(origin, async platform => (platform === 'windows-x64' ? readFile(cmd) : platform));
  const server = createServer((request, response) => {
    const file = path.join(origin, ...decodeURIComponent(new URL(request.url, 'http://local').pathname).split('/'));
    const stream = createReadStream(file);
    stream.on('error', () => { response.statusCode = 404; response.end(); });
    stream.pipe(response);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const live = `http://127.0.0.1:${server.address().port}`;
    const generate = (out, signer) => generateEntryScripts({
      version, setups, origins: [`${live}/missing`, live], signer, out,
    });
    const [, unsigned] = await generate(path.join(root, 'unsigned'), null);
    const [, wrongSigner] = await generate(path.join(root, 'signed'), '0'.repeat(40));
    // Keep download tests independent of a real setup installation on the test machine.
    // 让下载测试独立于测试机器上实际安装的安装器。
    const freshPrefix = `--prefix=${path.join(root, 'fresh')}`;
    const installedRoot = path.join(root, 'existing installation');
    const setupDir = path.join(installedRoot, 'setup');
    await mkdir(setupDir, { recursive: true });
    await writeFile(path.join(setupDir, 'installation.json'), '{}');
    await copyFile(cmd, path.join(setupDir, 'vulcan-code-setup.exe'));
    for (const shell of powerShells) {
      const run = (script, ...args) => runAsync(shell, ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', script, ...args]);
      const passed = await run(unsigned, '/d', '/c', 'echo', 'hello', freshPrefix);
      assert.equal(passed.status, 0, `${shell}: ${passed.stderr}`);
      assert.match(passed.stdout, /hello/);
      assert.match(passed.stdout + passed.stderr, /trying the next origin/);
      // The China route downloads directly from the mirror and is forwarded to the installer.
      // 中国大陆线路直接从镜像下载，并将选项透传给安装器。
      const china = await run(unsigned, '/d', '/c', 'echo', 'hello', freshPrefix, '--route=china');
      assert.equal(china.status, 0, `${shell}: ${china.stderr}`);
      assert.match(china.stdout, /--route=china/);
      assert.doesNotMatch(china.stdout + china.stderr, /\/missing|trying the next origin/);
      const international = await run(unsigned, '/d', '/c', 'echo', 'hello', freshPrefix, '--route', 'international');
      assert.notEqual(international.status, 0, shell);
      assert.doesNotMatch(international.stdout + international.stderr, /trying the next origin/);
      const failed = await run(unsigned, '/d', '/c', 'exit /b 5', '&', 'rem', freshPrefix);
      assert.notEqual(failed.status, 0, shell);
      assert.match(failed.stderr + failed.stdout, /exited with 5/);
      // A cancelled setup program (CANCELLED_EXIT_CODE) changed nothing, so the script ends without a failure.
      // 被取消的安装器（CANCELLED_EXIT_CODE）未做任何改动，因此脚本结束时不报告失败。
      const cancelled = await run(unsigned, '/d', '/c', 'exit /b 130', '&', 'rem', freshPrefix);
      assert.equal(cancelled.status, 0, `${shell}: ${cancelled.stderr}`);
      assert.doesNotMatch(cancelled.stderr + cancelled.stdout, /exited with/);
      const refused = await run(wrongSigner, '/d', '/c', 'echo', 'hello', freshPrefix);
      assert.notEqual(refused.status, 0, shell);
      assert.doesNotMatch(refused.stdout, /hello/);
      assert.match(refused.stderr + refused.stdout, /not signed by the Vulcan Code certificate/);
      const reused = await run(wrongSigner, '/d', '/c', 'echo', 'installed', '--prefix', installedRoot);
      assert.equal(reused.status, 0, `${shell}: ${reused.stderr}`);
      assert.match(reused.stdout, /installed/);
      assert.doesNotMatch(reused.stdout, /Downloading/);
      // After the setup program, the session that ran the script has every user PATH entry; the
      // user PATH is only read here.
      // 安装器运行后，运行脚本的会话包含每个用户 PATH 条目；这里只读取用户 PATH。
      const session = await runAsync(shell, ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', [
        `$env:Path = "$env:SystemRoot\\System32"`,
        `& '${unsigned.replaceAll("'", "''")}' /d /c echo hello '${freshPrefix.replaceAll("'", "''")}'`,
        `$expected = ([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ } | ForEach-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\\') }) -join '|'`,
        `"EXPECTED=$expected"`,
        `"SESSION=$env:Path"`,
      ].join('; ')]);
      assert.equal(session.status, 0, `${shell}: ${session.stderr}`);
      const line = name => session.stdout.split(/\r?\n/).find(entry => entry.startsWith(`${name}=`))?.slice(name.length + 1) ?? '';
      const sessionEntries = line('SESSION').split(';').map(entry => entry.replace(/\\+$/, '').toLowerCase());
      for (const entry of line('EXPECTED').split('|').filter(Boolean)) {
        assert.ok(sessionEntries.includes(entry.toLowerCase()), `${shell}: ${entry} is missing from the session PATH`);
      }
    }
    await copyFile(path.join(setups, setupProgramName(version, 'linux-x64')), path.join(setups, setupProgramName(version, 'windows-x64')));
    const tampered = await runAsync(powerShells[0], ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', unsigned, '/d', '/c', 'echo', 'hello', freshPrefix]);
    assert.notEqual(tampered.status, 0);
    assert.match(tampered.stderr + tampered.stdout, /does not match its published SHA-256/);
  } finally {
    server.close();
  }
}));
