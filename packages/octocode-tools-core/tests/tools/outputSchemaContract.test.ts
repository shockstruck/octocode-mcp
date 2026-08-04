import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import path from 'node:path';
import { z } from 'zod';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import type { CallToolResult } from '@modelcontextprotocol/sdk/types.js';
import { resetPathValidator } from '@octocodeai/octocode-engine/pathValidator';

import { executeDirectTool } from '../../src/tools/directToolCatalog.js';
import { executeBulkOperation } from '../../src/utils/response/bulk.js';
import { buildGhSearchCodeFinalizer } from '../../src/tools/github_search_code/finalizer.js';
import { buildGithubFetchContentFinalizer } from '../../src/tools/github_fetch_content/finalizer.js';
import type {
  FlatQueryResult,
  ProcessedBulkResult,
} from '../../src/types/toolResults.js';
import { cleanup } from '../../src/serverConfig.js';
import { setRuntimeSurface, _resetRuntimeSurface } from '@octocodeai/config';

// Output schemas (the contract the MCP server advertises + the SDK validates).
import { GitHubCodeSearchOutputLocalSchema } from '../../src/tools/github_search_code/scheme.js';
import { GitHubFetchContentOutputLocalSchema } from '../../src/tools/github_fetch_content/scheme.js';
import { GitHubSearchRepositoriesOutputLocalSchema } from '../../src/tools/github_search_repos/scheme.js';
import { GitHubSearchPullRequestsOutputLocalSchema } from '../../src/tools/github_search_pull_requests/scheme.js';
import { GitHubViewRepoStructureOutputLocalSchema } from '../../src/tools/github_view_repo_structure/scheme.js';
import { GitHubCloneRepoOutputLocalSchema } from '../../src/tools/github_clone_repo/scheme.js';
import { NpmSearchOutputLocalSchema } from '../../src/tools/package_search/scheme.js';
import { LocalSearchCodeOutputSchema } from '../../src/tools/local_ripgrep/scheme.js';
import { LocalFindFilesOutputSchema } from '../../src/tools/local_find_files/scheme.js';
import { LocalViewStructureOutputSchema } from '../../src/tools/local_view_structure/scheme.js';
import { LocalGetFileContentOutputSchema } from '../../src/tools/local_fetch_content/scheme.js';
import { LocalBinaryInspectOutputSchema } from '../../src/tools/local_binary_inspect/scheme.js';
import { LspGetSemanticsOutputSchema } from '../../src/tools/lsp/semantic_content/scheme.js';

// The repo root, so local-tool queries resolve against real files. The vitest
// cwd is the tools-core package dir.
const PKG_DIR = process.cwd();
const REPO_ROOT = path.resolve(PKG_DIR, '..', '..');
const BINARY_FIXTURE = path.join(
  REPO_ROOT,
  '.yarn',
  'releases',
  'yarn-4.9.1.cjs'
);
const MISSING_LOCAL_PATH = path.join(
  REPO_ROOT,
  '__octocode_output_contract_missing__'
);
const OUTSIDE_LOCAL_PATH = path.resolve(
  REPO_ROOT,
  '..',
  '__octocode_output_contract_outside__'
);

type StructuredContent = Record<string, unknown>;

/**
 * The SDK validates structuredContent against outputSchema on every NON-error
 * result (isError:true is exempt). A drifted schema turns a good result into a
 * runtime error, so these tests assert the advertised JSON Schema accepts the
 * emitted structuredContent through a real MCP client.
 */
function structuredOf(result: {
  structuredContent?: unknown;
}): StructuredContent {
  expect(result.structuredContent).toBeTypeOf('object');
  return result.structuredContent as StructuredContent;
}

async function expectMcpOutputContract(
  toolName: string,
  outputSchema: z.ZodType,
  result: CallToolResult
): Promise<void> {
  structuredOf(result);

  const server = new McpServer({ name: 'contract-server', version: '0.0.0' });
  const client = new Client({ name: 'contract-client', version: '0.0.0' });
  const [serverTransport, clientTransport] =
    InMemoryTransport.createLinkedPair();

  server.registerTool(
    toolName,
    {
      inputSchema: {},
      outputSchema: outputSchema as never,
    },
    async () => result
  );

  await Promise.all([
    server.connect(serverTransport),
    client.connect(clientTransport),
  ]);

  try {
    const listed = await client.listTools();
    expect(
      listed.tools.find(tool => tool.name === toolName)?.outputSchema
    ).toBeDefined();

    const validated = await client.callTool({ name: toolName, arguments: {} });
    expect(validated.isError).toBeFalsy();
  } finally {
    await client.close();
    await server.close();
  }
}

async function expectMcpErrorOutputContract(
  toolName: string,
  outputSchema: z.ZodType,
  result: CallToolResult,
  expectedData: Record<string, unknown>
): Promise<void> {
  expect(result.isError).toBe(true);

  const structured = structuredOf(result);
  const results = structured.results as Array<Record<string, unknown>>;
  expect(results).toHaveLength(1);
  expect(results[0]?.status).toBe('error');

  const data = results[0]?.data as Record<string, unknown>;
  expect(data.error).toBeTypeOf('string');
  expect(data).toMatchObject(expectedData);
  expect(Object.keys(data).sort()).toEqual(
    ['error', ...Object.keys(expectedData)].sort()
  );

  // Context Forge validates structuredContent even when the MCP result is an
  // error. Force the SDK down its equivalent validation path for this test.
  await expectMcpOutputContract(toolName, outputSchema, {
    ...result,
    isError: false,
  });
}

describe('MCP outputSchema contract — client validates structuredContent', () => {
  const originalEnableLocal = process.env.ENABLE_LOCAL;
  const originalAllowedPaths = process.env.ALLOWED_PATHS;
  const originalWorkspaceRoot = process.env.WORKSPACE_ROOT;

  beforeAll(() => {
    setRuntimeSurface('mcp');
    process.env.ENABLE_LOCAL = 'true';
    process.env.ALLOWED_PATHS = REPO_ROOT;
    process.env.WORKSPACE_ROOT = REPO_ROOT;
    resetPathValidator({ workspaceRoot: REPO_ROOT, includeHomeDir: false });
    cleanup();
  });

  afterAll(() => {
    if (originalEnableLocal === undefined) delete process.env.ENABLE_LOCAL;
    else process.env.ENABLE_LOCAL = originalEnableLocal;
    if (originalAllowedPaths === undefined) delete process.env.ALLOWED_PATHS;
    else process.env.ALLOWED_PATHS = originalAllowedPaths;
    if (originalWorkspaceRoot === undefined) delete process.env.WORKSPACE_ROOT;
    else process.env.WORKSPACE_ROOT = originalWorkspaceRoot;
    resetPathValidator();
    _resetRuntimeSurface();
    cleanup();
  });

  // ----------------------------------------------------------------------
  // Local tools — run the exact MCP path (executeDirectTool) against the real
  // repo, so the structuredContent is genuinely what the server would emit.
  // ----------------------------------------------------------------------

  it('localSearchCode', async () => {
    const result = await executeDirectTool('localSearchCode', {
      queries: [
        {
          path: path.join(PKG_DIR, 'src', 'utils', 'response'),
          keywords: 'sanitizeStructuredContent',
          maxFiles: 5,
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpOutputContract(
      'localSearchCode',
      LocalSearchCodeOutputSchema,
      result
    );
  });

  it('localFindFiles', async () => {
    const result = await executeDirectTool('localFindFiles', {
      queries: [
        {
          path: path.join(PKG_DIR, 'src', 'utils', 'response'),
          namePattern: '*.ts',
          maxDepth: 2,
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpOutputContract(
      'localFindFiles',
      LocalFindFilesOutputSchema,
      result
    );
  });

  it('localViewStructure', async () => {
    const result = await executeDirectTool('localViewStructure', {
      queries: [
        {
          path: path.join(PKG_DIR, 'src'),
          maxDepth: 1,
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpOutputContract(
      'localViewStructure',
      LocalViewStructureOutputSchema,
      result
    );
  });

  it('localViewStructure — missing allowed path', async () => {
    const result = await executeDirectTool('localViewStructure', {
      queries: [
        {
          path: MISSING_LOCAL_PATH,
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpErrorOutputContract(
      'localViewStructure',
      LocalViewStructureOutputSchema,
      result,
      {
        errorCode: 'pathValidationFailed',
        toolName: 'localViewStructure',
      }
    );
  });

  it('localViewStructure — outside allowed path', async () => {
    const result = await executeDirectTool('localViewStructure', {
      queries: [
        {
          path: OUTSIDE_LOCAL_PATH,
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpErrorOutputContract(
      'localViewStructure',
      LocalViewStructureOutputSchema,
      result,
      {
        errorCode: 'pathValidationFailed',
        toolName: 'localViewStructure',
        cwd: REPO_ROOT,
        resolvedPath: OUTSIDE_LOCAL_PATH,
      }
    );
  });

  it('localGetFileContent', async () => {
    const result = await executeDirectTool('localGetFileContent', {
      queries: [
        {
          path: path.join(PKG_DIR, 'package.json'),
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpOutputContract(
      'localGetFileContent',
      LocalGetFileContentOutputSchema,
      result
    );
  });

  it('localGetFileContent — missing allowed path', async () => {
    const result = await executeDirectTool('localGetFileContent', {
      queries: [
        {
          path: MISSING_LOCAL_PATH,
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpErrorOutputContract(
      'localGetFileContent',
      LocalGetFileContentOutputSchema,
      result,
      {
        errorCode: 'fileAccessFailed',
        toolName: 'localGetFileContent',
        resolvedPath: MISSING_LOCAL_PATH,
      }
    );
  });

  it('localGetFileContent — outside allowed path', async () => {
    const result = await executeDirectTool('localGetFileContent', {
      queries: [
        {
          path: OUTSIDE_LOCAL_PATH,
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpErrorOutputContract(
      'localGetFileContent',
      LocalGetFileContentOutputSchema,
      result,
      {
        errorCode: 'pathValidationFailed',
        toolName: 'localGetFileContent',
        cwd: REPO_ROOT,
        resolvedPath: OUTSIDE_LOCAL_PATH,
      }
    );
  });

  it('localBinaryInspect', async () => {
    const result = await executeDirectTool('localBinaryInspect', {
      queries: [
        {
          path: BINARY_FIXTURE,
          mode: 'inspect',
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpOutputContract(
      'localBinaryInspect',
      LocalBinaryInspectOutputSchema,
      result
    );
  });

  it('lspGetSemantics — resolved definition', async () => {
    const result = await executeDirectTool('lspGetSemantics', {
      queries: [
        {
          uri: path.join(PKG_DIR, 'src', 'utils', 'response', 'bulk.ts'),
          symbolName: 'executeBulkOperation',
          mode: 'definition',
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    await expectMcpOutputContract(
      'lspGetSemantics',
      LspGetSemanticsOutputSchema,
      result
    );
  });

  it('lspGetSemantics — symbolNotFound (lsp field omitted)', async () => {
    const result = await executeDirectTool('lspGetSemantics', {
      queries: [
        {
          uri: path.join(PKG_DIR, 'src', 'utils', 'response', 'bulk.ts'),
          symbolName: 'noSuchSymbolAnywhere1234',
          mode: 'definition',
          mainResearchGoal: 'contract test',
          researchGoal: 'contract test',
          reasoning: 'contract test',
        },
      ],
    });
    expect(
      LspGetSemanticsOutputSchema.parse(structuredOf(result))
    ).toBeDefined();
  });

  // ----------------------------------------------------------------------
  // GitHub code search / fetch content — drive the real finalizers (which run
  // cleanJsonObject -> sanitize via formatFinalizedResponse) with fixtures
  // shaped like real provider output. No network.
  // ----------------------------------------------------------------------

  it('ghSearchCode', async () => {
    const finalize = buildGhSearchCodeFinalizer();
    const out = finalize({
      queries: [{ id: 'q1', keywords: ['foo'] }] as never,
      results: [
        {
          id: 'q1',
          status: 'success',
          data: {
            results: [
              {
                id: 'octo/repo',
                owner: 'octo',
                repo: 'repo',
                matches: [{ path: 'src/a.ts', value: 'foo bar' }],
              },
            ],
            pagination: {
              currentPage: 1,
              totalPages: 1,
              hasMore: false,
            },
          },
        },
      ] as unknown as FlatQueryResult[],
      config: {} as never,
    });
    await expectMcpOutputContract(
      'ghSearchCode',
      GitHubCodeSearchOutputLocalSchema,
      out
    );
  });

  it('ghSearchCode — empty + error rows', () => {
    const finalize = buildGhSearchCodeFinalizer();
    const out = finalize({
      queries: [{ id: 'q1' }, { id: 'q2' }] as never,
      results: [
        {
          id: 'q1',
          status: 'success',
          data: { results: [], incompleteResults: true },
        },
        { id: 'q2', status: 'error', data: { error: 'boom' } },
      ] as unknown as FlatQueryResult[],
      config: {} as never,
    });
    expect(
      GitHubCodeSearchOutputLocalSchema.parse(structuredOf(out))
    ).toBeDefined();
  });

  it('ghGetFileContent', async () => {
    const finalize = buildGithubFetchContentFinalizer();
    const out = finalize({
      queries: [
        { id: 'q1', owner: 'octo', repo: 'repo', path: 'README.md' },
      ] as never,
      results: [
        {
          id: 'q1',
          status: 'success',
          data: {
            path: 'README.md',
            content: '# Hello',
            fileSize: 7,
            totalLines: 1,
            startLine: 1,
            endLine: 1,
          },
        },
      ] as unknown as FlatQueryResult[],
      config: {} as never,
    });
    await expectMcpOutputContract(
      'ghGetFileContent',
      GitHubFetchContentOutputLocalSchema,
      out
    );
  });

  it('ghGetFileContent — error row', () => {
    const finalize = buildGithubFetchContentFinalizer();
    const out = finalize({
      queries: [
        { id: 'q1', owner: 'octo', repo: 'repo', path: 'missing.md' },
      ] as never,
      results: [
        { id: 'q1', status: 'error', data: { error: 'not found' } },
      ] as unknown as FlatQueryResult[],
      config: {} as never,
    });
    // Error-only finalizer output is isError; the SDK exempts it, but the
    // structuredContent should still parse.
    expect(
      GitHubFetchContentOutputLocalSchema.parse(structuredOf(out))
    ).toBeDefined();
  });

  // ----------------------------------------------------------------------
  // GitHub repos / PRs / structure / clone and npm — these use the generic
  // (no-finalize) bulk path. Driving executeBulkOperation with a fixture
  // processor exercises the identical envelope + cleanJson -> sanitize path.
  // ----------------------------------------------------------------------

  async function runBulk(
    toolName: string,
    keysPriority: string[],
    data: Record<string, unknown>
  ) {
    const processor = async (): Promise<ProcessedBulkResult> => ({
      status: undefined,
      ...data,
    });
    return executeBulkOperation([{ id: 'q1' }], processor, {
      toolName,
      keysPriority,
    });
  }

  it('ghSearchRepos', async () => {
    const result = await runBulk(
      'ghSearchRepos',
      ['repositories', 'pagination', 'error'],
      {
        repositories: [
          {
            owner: 'octo',
            repo: 'repo',
            url: 'https://github.com/octo/repo',
            stars: 10,
            language: 'TypeScript',
            description: 'a repo',
          },
        ],
        pagination: { currentPage: 1, totalPages: 1, hasMore: false },
      }
    );
    await expectMcpOutputContract(
      'ghSearchRepos',
      GitHubSearchRepositoriesOutputLocalSchema,
      result
    );
  });

  it('ghHistoryResearch (pull requests)', async () => {
    const result = await runBulk(
      'ghHistoryResearch',
      ['pull_requests', 'pagination', 'error'],
      {
        pull_requests: [
          {
            number: 1,
            title: 'a PR',
            url: 'https://github.com/octo/repo/pull/1',
            state: 'closed',
            merged: true,
            author: 'octo',
          },
        ],
        total_count: 1,
        pagination: { currentPage: 1, totalPages: 1, hasMore: false },
      }
    );
    await expectMcpOutputContract(
      'ghHistoryResearch',
      GitHubSearchPullRequestsOutputLocalSchema,
      result
    );
  });

  it('ghViewRepoStructure', async () => {
    const result = await runBulk(
      'ghViewRepoStructure',
      ['entries', 'pagination', 'error'],
      {
        entries: [
          { path: 'src', name: 'src', type: 'dir' },
          { path: 'README.md', name: 'README.md', type: 'file', size: 100 },
        ],
        pagination: { currentPage: 1, totalPages: 1, hasMore: false },
      }
    );
    await expectMcpOutputContract(
      'ghViewRepoStructure',
      GitHubViewRepoStructureOutputLocalSchema,
      result
    );
  });

  it('ghCloneRepo', async () => {
    const result = await runBulk(
      'ghCloneRepo',
      ['localPath', 'resolvedBranch', 'error'],
      {
        localPath: '/tmp/repo',
        resolvedBranch: 'main',
        cached: false,
        location: {
          kind: 'repo',
          localPath: '/tmp/repo',
          source: 'clone',
        },
      }
    );
    await expectMcpOutputContract(
      'ghCloneRepo',
      GitHubCloneRepoOutputLocalSchema,
      result
    );
  });

  it('npmSearch', async () => {
    const result = await runBulk(
      'npmSearch',
      ['packages', 'pagination', 'error'],
      {
        packages: [
          {
            name: 'react',
            version: '18.0.0',
            license: 'MIT',
            description: 'A library',
            downloads: 1000,
          },
        ],
      }
    );
    await expectMcpOutputContract(
      'npmSearch',
      NpmSearchOutputLocalSchema,
      result
    );
  });
});
