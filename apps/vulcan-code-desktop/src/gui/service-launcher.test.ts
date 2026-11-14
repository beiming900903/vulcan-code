// Exercise service readiness against a real isolated HTTP discovery endpoint.
// 使用真实隔离 HTTP 发现端点验证服务就绪行为。
import { strict as assert } from 'node:assert';
import { writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { mkdtemp, readFile, writeFile, unlink, rmdir } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { ensureServiceInstance, readLiveServiceInstance, resolveWebUiUrl, type ServiceLaunchDescription } from './service-launcher.js';

// Run readiness through the public launcher with supplied sequential health states.
// 通过公开启动器，使用给定的连续健康状态执行就绪等待。
// statuses controls server replies; body checks the final result while cleanup always releases resources.
// statuses 控制服务应答；body 检查最终结果，清理始终释放资源。
async function withHealth(
  statuses: string[],
  body: (description: ServiceLaunchDescription, states: string[]) => Promise<void>,
  serveSnapshot?: import('node:http').RequestListener,
): Promise<void> {
  // Every test owns its discovery file and ephemeral port.
  // 每项测试拥有独立发现文件和临时端口。
  const directory = await mkdtemp(path.join(os.tmpdir(), 'vulcan-launcher-test-'));
  const discoveryPath = path.join(directory, 'discovery.json');
  const states: string[] = [];
  let baseUrl = '';
  const identity = { pid: process.pid, serviceToken: 'test-only', startedAt: '1', host: '127.0.0.1', port: 0 };
  const server = createServer((request, response) => {
    if (request.url === '/rpc') {
      if (serveSnapshot) serveSnapshot(request, response);
      else response.writeHead(404).end();
      return;
    }
    const status = statuses[Math.min(states.length, statuses.length - 1)]!;
    states.push(status);
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify({ ...identity, baseUrl, status }));
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert(address && typeof address !== 'string');
  identity.port = address.port;
  baseUrl = `http://127.0.0.1:${address.port}`;
  await writeFile(discoveryPath, JSON.stringify({ ...identity, baseUrl, status: 'starting' }));
  const description: ServiceLaunchDescription = {
    protocolVersion: 1, serviceName: 'test', serviceVersion: 'test', profile: 'debug',
    executablePath: 'must-not-spawn', dataRoot: directory, discoveryPath,
    lockPath: path.join(directory, 'lock'), defaultWebUiMode: 'vite', viteUrl: null, modes: [],
  };
  try { await body(description, states); }
  finally {
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    await unlink(discoveryPath);
    await rmdir(directory);
  }
}

test('waits for business readiness without spawning a second live owner', async () => {
  await withHealth(['starting', 'starting', 'ready'], async (description, states) => {
    const stages: string[] = [];
    const instance = await ensureServiceInstance(description, [], progress => stages.push(progress.stage));
    assert.equal(instance.status, 'ready');
    assert.equal(states.length, 3);
    assert(!stages.includes('starting-service'));
  });
});

test('forwards authenticated Core startup steps before readiness without exposing credentials', async () => {
  // Cover both a download without a length and a subsequent measured download.
  // 覆盖总量未知的下载，以及随后可测量的下载。
  const steps = [
    { id: 'luaskills', label: 'LuaSkills', status: 'running', progressPhase: 'downloading', skillId: 'fixture' },
    { id: 'luaskills', label: 'LuaSkills', status: 'running', progressPhase: 'downloading', skillId: 'fixture', progressPercent: 31 },
    { id: 'mcp', label: 'MCP Runtime', status: 'running' },
  ];
  // Count startup RPCs independently of the health probes.
  // 独立于健康探测统计启动 RPC。
  let probes = 0;
  await withHealth(['starting', 'starting', 'starting', 'starting', 'ready'], async (description) => {
    // Capture the renderer projection through the public launcher reporter.
    // 通过公开启动器上报接口捕获渲染投影。
    const snapshots: unknown[] = [];
    await ensureServiceInstance(description, [], progress => {
      if (progress.snapshot) snapshots.push(progress.snapshot.steps[0]);
      assert(!JSON.stringify(progress).includes('test-only'));
    });
    assert.deepEqual(snapshots, steps);
  }, (request, response) => {
    assert.equal(request.method, 'POST');
    assert.equal(request.headers['x-vulcan-service-token'], 'test-only');
    // Verify the request uses the existing startup method.
    // 校验请求使用现有启动方法。
    let body = '';
    request.setEncoding('utf8').on('data', chunk => { body += chunk; }).on('end', () => {
      assert.deepEqual(JSON.parse(body), { method: 'startup.snapshot' });
      response.end(JSON.stringify({ ok: true, result: { ready: false, phase: 'bootstrapping', steps: [steps[probes++]], recentLogs: ['test-only'] } }));
    });
  });
});

for (const status of ['failed', 'stopping']) {
  test(`refuses an existing ${status} owner`, async () => {
    await withHealth([status], async description => {
      await assert.rejects(ensureServiceInstance(description, [], () => {}), new RegExp(status));
    });
  });
}

// A starting owner that vanishes before readiness must trigger this GUI's own launch instead of waiting out the deadline.
// 启动中的所有者在就绪前消失时，必须触发本 GUI 自行启动，而不是等满截止时间。
test('launches its own service when a starting owner disappears', async () => {
  await withHealth(['starting'], async (description, states) => {
    const original = JSON.parse(await readFile(description.discoveryPath, 'utf8')) as { baseUrl: string };
    const stages: string[] = [];
    const pending = ensureServiceInstance(description, [], (progress) => {
      stages.push(progress.stage);
      // The owner vanishes right after the first probe observed it starting.
      // 首次探测观察到启动中之后，所有者立即消失。
      if (progress.stage === 'awaiting-ready') {
        writeFileSync(description.discoveryPath, JSON.stringify({ ...original, baseUrl: `${original.baseUrl}/gone` }));
      }
    });
    // Check the missing-executable failure independently of the desktop display language.
    // 检查可执行文件不存在这一失败原因，避免断言依赖桌面显示语言。
    await assert.rejects(pending, /spawn must-not-spawn ENOENT/);
    assert.equal(states.length, 1);
    assert(stages.includes('starting-service'));
  });
});

// Rejects a rewritten discovery origin before the GUI can load it as a privileged workbench.
// 在 GUI 将改写的发现来源作为特权工作台加载前拒绝它。
test('rejects a rewritten discovery address before probing health', async () => {
  await withHealth(['ready'], async (description, states) => {
    const original = JSON.parse(await readFile(description.discoveryPath, 'utf8')) as { baseUrl: string };
    for (const baseUrl of [`${original.baseUrl}/rewritten`, 'https://example.invalid']) {
      await writeFile(description.discoveryPath, JSON.stringify({ ...original, baseUrl }));
      assert.equal(await readLiveServiceInstance(description.discoveryPath), null);
    }
    assert.equal(states.length, 0);
  });
});

// An explicit workbench override must remain inside the selected Core's trusted pages.
// 显式工作台覆盖必须保持在所选 Core 的可信页面内。
test('accepts only the selected Core or its declared Vite page as an explicit WebUI', async () => {
  await withHealth(['ready'], async description => {
    const instance = await readLiveServiceInstance(description.discoveryPath);
    assert(instance);
    assert.equal(resolveWebUiUrl(description, instance, instance.baseUrl), instance.baseUrl);
    description.viteUrl = 'http://127.0.0.1:17601';
    assert.equal(resolveWebUiUrl(description, instance, description.viteUrl), description.viteUrl);
    assert.throws(() => resolveWebUiUrl(description, instance, 'https://example.invalid'),
      /must match the selected Core/);
    assert.throws(() => resolveWebUiUrl(description, instance, `${instance.baseUrl}/rewritten`),
      /must match the selected Core/);
  });
});
