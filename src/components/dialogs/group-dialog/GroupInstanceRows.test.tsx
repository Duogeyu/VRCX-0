import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';

vi.mock('@/components/LocationWorld', async () => {
    const React = await import('react');

    return {
        LocationWorld: ({
            instanceClickAction
        }: {
            instanceClickAction?: string;
        }) =>
            React.createElement('span', {
                'data-instance-click-action': instanceClickAction
            })
    };
});

vi.mock('@/components/instances/InstanceActionBar', () => ({
    InstanceActionBar: () => null
}));

vi.mock('../world-dialog/WorldDialogViewParts', () => ({
    InstanceUserTiles: () => null
}));

import { GroupInstanceRows } from './GroupInstanceRows';

describe('GroupInstanceRows', () => {
    it('opens the instance details instead of the launch dialog from the instance name', () => {
        const html = renderToStaticMarkup(
            React.createElement(GroupInstanceRows, {
                currentUserId: 'usr_current',
                instances: [
                    {
                        friendCount: 0,
                        id: '12345',
                        instanceId: '12345',
                        location: 'wrld_test:12345~group(grp_test)',
                        ref: {},
                        tag: 'wrld_test:12345~group(grp_test)',
                        users: [],
                        worldId: 'wrld_test',
                        worldName: 'Test World'
                    }
                ]
            })
        );

        expect(html).toContain('data-instance-click-action="world"');
    });

    /**
     * 来源说明挂在**每一条自己的卡片里**，跟着 `origin` 走。
     *
     * 一个列表里可能混着 VRChat 的实时房间和星遇的社团房间，底部一句总说明
     * 说不清「哪几条不是实时的」。这里逐个断言两种行各自的样子。
     */
    it('只给星遇社团来的房间挂来源说明', () => {
        const base = {
            friendCount: 0,
            ref: {},
            users: [],
            worldId: 'wrld_test'
        };
        const html = renderToStaticMarkup(
            React.createElement(GroupInstanceRows, {
                currentUserId: 'usr_current',
                instances: [
                    {
                        ...base,
                        id: 'a',
                        instanceId: 'a',
                        location: 'wrld_test:a~group(grp_test)',
                        tag: 'wrld_test:a~group(grp_test)',
                        origin: 'vrcc'
                    },
                    {
                        ...base,
                        id: 'b',
                        instanceId: 'b',
                        location: 'wrld_test:b~group(grp_test)',
                        tag: 'wrld_test:b~group(grp_test)',
                        origin: 'vrchat'
                    }
                ]
            })
        );

        // 两条房间都在（`LocationWorld` 在这个测试里是桩，所以数行数而不是
        // 数 location 字符串），但说明只出现一次——只有星遇那一条。
        expect(
            (html.match(/data-instance-click-action="world"/g) ?? []).length
        ).toBe(2);
        const noteCount = (
            html.match(/dialog\.group\.vrcc\.rooms_source/g) ?? []
        ).length;
        expect(noteCount).toBe(1);
    });
});
