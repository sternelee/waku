import { describe, expect, test } from 'bun:test';
import type { WakuClient } from '@waku/client';

import { browseDaemonDirectory, createProject, persistProject } from './daemon-api';

describe('mobile daemon API', () => {
  test('browses a directory on the remote host', async () => {
    let command: unknown;
    const directory = {
      type: 'directory' as const,
      path: '/Users/me',
      parent: '/Users',
      home: '/Users/me',
      filesystem_root: '/',
      entries: [],
    };
    const client = {
      request: async (next: unknown) => {
        command = next;
        return { type: 'workspace', result: directory };
      },
    } as unknown as WakuClient;

    await expect(browseDaemonDirectory(client, null)).resolves.toEqual(directory);
    expect(command).toEqual({
      type: 'workspace',
      operation: { type: 'browseDirectory', path: null },
    });
  });

  test('normalizes absolute Unix and Windows project paths', () => {
    expect(createProject('/srv/waku/', 'one', 10)).toEqual({
      id: 'one',
      name: 'waku',
      path: '/srv/waku',
      created_at: 10,
    });
    expect(createProject('C:\\dev\\waku\\', 'two', 10).name).toBe('waku');
    expect(() => createProject('dev/waku', 'three')).toThrow('absolute path');
  });

  test('persists a project without replacing sessions', async () => {
    const commands: unknown[] = [];
    const client = {
      request: async (command: any) => {
        commands.push(command);
        if (command.type === 'loadTaskState') {
          return {
            type: 'taskState',
            revision: 2,
            projects: [],
            sessions: [{ id: 'live' }],
          };
        }
        return { type: 'taskStateSaved', revision: 3, sessions: [] };
      },
    } as unknown as WakuClient;
    const project = createProject('/srv/waku', 'project', 10);

    const saved = await persistProject(client, project);
    expect(saved.project).toEqual(project);
    expect(commands[1]).toEqual({
      type: 'saveTaskState',
      projects: [project],
      liveSessionIds: ['live'],
      sessions: [],
    });
  });
});
