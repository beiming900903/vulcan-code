// Fill the install.sh and install.ps1 templates for one setup version: the version, the download origins,
// the SHA-256 of every platform's setup program and the Windows signer (plan section 18.6). The filled
// scripts are published beside the programs in `setup/<version>/` and listed by the setup pointer;
// openvulcan-web copies the ones the stable setup pointer names to the top level.
// 为一个安装器版本填写 install.sh 与 install.ps1 模板：版本、下载源、各平台安装器的 SHA-256 与 Windows
// 签名者（方案 18.6 节）。填好的脚本与程序一同发布在 `setup/<版本>/` 并由安装器指针列出；openvulcan-web
// 把 stable 安装器指针所列的脚本复制到顶层。
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { chmod, mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { releasePlatforms } from './desktop-runtime.mjs';

// The repository is resolved from this script rather than the caller's working directory.
// 仓库由脚本位置确定，不依赖调用方工作目录。
const repository = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// entryScripts are the templates in scripts/, published under the same names; the setup pointer of
// vulcan-install-core (`setup::ENTRY_SCRIPTS`) lists exactly these names.
// entryScripts 是 scripts/ 中的模板，以相同名称发布；vulcan-install-core 的安装器指针
// （`setup::ENTRY_SCRIPTS`）只列出这些名称。
export const entryScripts = Object.freeze(['install.sh', 'install.ps1']);

// releaseVersionPattern accepts the version forms releases use.
// releaseVersionPattern 接受发行版本使用的版本形式。
const releaseVersionPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?$/;

// setupProgramName names a platform's setup program as the setup pointer requires (vulcan-install-core
// `setup::program_file`).
// setupProgramName 按安装器指针的要求命名某平台的安装器（vulcan-install-core `setup::program_file`）。
export function setupProgramName(version, platform) {
  if (!releaseVersionPattern.test(version)) throw new Error(`Invalid release version: ${version}.`);
  if (!releasePlatforms.includes(platform)) throw new Error(`Unknown release platform: ${platform}.`);
  return `vulcan-code-setup-${version}-${platform}${platform.startsWith('windows-') ? '.exe' : ''}`;
}

// digestKey is the template placeholder of a platform's setup program digest.
// digestKey 是某平台安装器摘要在模板中的占位符名称。
function digestKey(platform) {
  return `SHA256_${platform.toUpperCase().replaceAll('-', '_')}`;
}

// renderTemplate replaces every @@NAME@@ in template; a placeholder without a value, or a value without a
// placeholder, is an error so a template and its generator cannot drift apart.
// renderTemplate 替换模板中的每个 @@NAME@@；缺少取值的占位符或没有占位符的取值都是错误，使模板与生成器不会
// 彼此脱节。
export function renderTemplate(template, values) {
  const used = new Set();
  const rendered = template.replace(/@@([A-Z0-9_]+)@@/g, (_, name) => {
    if (!Object.hasOwn(values, name)) throw new Error(`The template needs a value for ${name}.`);
    used.add(name);
    return values[name];
  });
  const unused = Object.keys(values).filter(name => !used.has(name));
  if (unused.length > 0) throw new Error(`The template has no place for ${unused.join(', ')}.`);
  if (rendered.includes('@@')) throw new Error('The template holds a malformed placeholder.');
  return rendered;
}

// sha256File streams one file into its hex digest.
// sha256File 流式计算一个文件的十六进制摘要。
async function sha256File(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

// checkOrigin accepts an origin that both scripts can embed in single quotes and split on spaces.
// checkOrigin 接受两个脚本都能嵌入单引号并按空格切分的下载源。
function checkOrigin(origin) {
  if (!/^[a-z]+:\/\/[^\s'"`$\\]+$/.test(origin) || origin.endsWith('/')) {
    throw new Error(`Invalid download origin: ${origin}.`);
  }
  return origin;
}

// generateEntryScripts writes both filled scripts into out and returns their paths. setups holds every
// platform's setup program under its release name; signer is the certificate SHA-1 thumbprint install.ps1
// requires, or null for an unsigned test build.
// generateEntryScripts 把两个填好的脚本写入 out 并返回其路径。setups 以发行名称存放各平台安装器；signer 是
// install.ps1 要求的证书 SHA-1 指纹，未签名的测试构建为 null。
export async function generateEntryScripts({ version, setups, origins, signer, out }) {
  if (!releaseVersionPattern.test(version)) throw new Error(`Invalid release version: ${version}.`);
  // The release tool lists AUTO_ORIGINS in international, then mainland China order.
  // 发布工具按国际源、中国大陆镜像的顺序列出 AUTO_ORIGINS。
  if (!Array.isArray(origins) || origins.length !== 2 || origins[0] === origins[1]) {
    throw new Error('Expected two distinct download origins in international, then China order.');
  }
  origins.forEach(checkOrigin);
  if (signer !== null && !/^[0-9A-Fa-f]{40}$/.test(signer ?? '')) {
    throw new Error(`The signer must be a SHA-1 certificate thumbprint: ${signer}.`);
  }
  const digests = {};
  for (const platform of releasePlatforms) {
    digests[digestKey(platform)] = await sha256File(path.join(setups, setupProgramName(version, platform)));
  }
  const windows = digestKey('windows-x64');
  const values = {
    'install.sh': {
      VERSION: version,
      ORIGINS: origins.join(' '),
      ...Object.fromEntries(Object.entries(digests).filter(([name]) => name !== windows)),
    },
    'install.ps1': {
      VERSION: version,
      ORIGINS: origins.map(origin => `'${origin}'`).join(', '),
      [windows]: digests[windows],
      SIGNER: signer === null ? '' : signer.toUpperCase(),
    },
  };
  await mkdir(out, { recursive: true });
  const written = [];
  for (const name of entryScripts) {
    const rendered = renderTemplate(await readFile(path.join(repository, 'scripts', name), 'utf8'), values[name]);
    // Windows PowerShell 5.1 reads a file without a byte order mark in the system code page.
    // Windows PowerShell 5.1 按系统代码页读取没有字节顺序标记的文件。
    if (/[^\x00-\x7f]/.test(rendered) && name.endsWith('.ps1')) throw new Error(`${name} must stay ASCII.`);
    const target = path.join(out, name);
    await writeFile(`${target}.partial`, rendered);
    await chmod(`${target}.partial`, 0o755);
    await rename(`${target}.partial`, target);
    written.push(target);
  }
  return written;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, ...rest] = process.argv.slice(2);
  const usage = 'Usage: node scripts/entry-scripts.mjs generate --version V --setups DIR --origin INTERNATIONAL --origin CHINA '
    + '(--signer SHA1 | --unsigned) --out DIR';
  const options = { origins: [] };
  for (let index = 0; index < rest.length; index += 1) {
    const flag = rest[index];
    if (flag === '--unsigned') {
      options.unsigned = true;
      continue;
    }
    const value = rest[index + 1];
    index += 1;
    if (value === undefined) throw new Error(usage);
    if (flag === '--origin') options.origins.push(value);
    else if (['--version', '--setups', '--signer', '--out'].includes(flag) && !(flag.slice(2) in options)) {
      options[flag.slice(2)] = value;
    } else throw new Error(usage);
  }
  if (command !== 'generate' || !options.version || !options.setups || !options.out
    || Boolean(options.signer) === Boolean(options.unsigned)) {
    throw new Error(usage);
  }
  const written = await generateEntryScripts({
    version: options.version,
    setups: path.resolve(options.setups),
    origins: options.origins,
    signer: options.unsigned ? null : options.signer,
    out: path.resolve(options.out),
  });
  for (const file of written) console.log(file);
}
