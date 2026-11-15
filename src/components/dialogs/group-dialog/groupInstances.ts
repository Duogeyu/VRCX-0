import type {
    GroupDialogInstanceRow,
    GroupInstanceRecord,
    InstanceOrigin
} from '@/domain/entities/group';
import { parseInstanceOrigin } from '@/domain/entities/group';
import type { EntityRecord } from '@/domain/entities/shared';
import { presenceLiveInstanceTag, presenceOf } from '@/domain/friends/presence';
import type { FriendRosterById } from '@/domain/friends/types';
import { groupInstanceLocation } from '@/domain/instances/groupInstanceFacts';
import { parseLocation } from '@/shared/utils/location';
import { normalizeString } from '@/shared/utils/string';
import type { CurrentUserSnapshotState } from '@/state/runtimeStore';

/** 来源的权威排名：数字大的赢。加来源时**必须**在这里补一行。 */
const ORIGIN_RANK: Record<InstanceOrigin, number> = {
    vrchat: 100,
    vrcc: 10,
    unknown: 0
};

function originRank(origin: unknown): number {
    return ORIGIN_RANK[parseInstanceOrigin(origin)];
}

type InstanceUser = EntityRecord & {
    displayName?: string;
    id?: string;
    userId?: string;
};

interface MergeGroupInstancesOptions {
    groupId: string;
    friendsById:
        | FriendRosterById
        | Record<string, InstanceUser | null | undefined>;
    currentUserSnapshot: CurrentUserSnapshotState | null;
    currentLocation: string;
}

function isEntityRecord(value: unknown): value is EntityRecord {
    return Boolean(value && typeof value === 'object');
}

function entityRecord(value: unknown): EntityRecord | null {
    return isEntityRecord(value) ? value : null;
}

function entityRows(value: unknown): EntityRecord[] {
    return Array.isArray(value)
        ? value.filter((row): row is EntityRecord => Boolean(entityRecord(row)))
        : [];
}

/**
 * 两份 `users` 取并集，按 id 去重。
 *
 * 合并同一个 location 的两行时不能简单选一侧：好友是 `addUser` 之后才填的，
 * 那之前这一行已有的 `users` 可能已经攒了人（同源重复时尤其明显）。并集
 * 只会变多不会变少，任何一侧都不丢。
 */
function mergeUsers(left: unknown, right: unknown): EntityRecord[] {
    const seen = new Set<string>();
    const merged: EntityRecord[] = [];
    for (const user of [...entityRows(left), ...entityRows(right)]) {
        const id = normalizeString(user.id || user.userId);
        if (id) {
            if (seen.has(id)) {
                continue;
            }
            seen.add(id);
        }
        merged.push(user);
    }
    return merged;
}

export function normalizeLocation(value: unknown) {
    const normalized = typeof value === 'string' ? value.trim() : '';
    return normalized && normalized !== 'offline' && normalized !== 'private'
        ? normalized
        : '';
}

export function userGroupLocation(user: InstanceUser | null | undefined) {
    const presence = presenceOf(user);
    return presence
        ? presenceLiveInstanceTag(presence, { preferTraveling: true })
        : '';
}

export function instanceLocation(instance: GroupInstanceRecord) {
    const projectedLocation = entityRecord(instance.$location);
    const directLocation = normalizeLocation(
        groupInstanceLocation(instance) ||
            instance.tag ||
            projectedLocation?.tag
    );
    if (directLocation) {
        return directLocation;
    }
    const world = entityRecord(instance.world);
    const worldId = instance.worldId || world?.id || '';
    const instanceId =
        instance.instanceId || instance.id || instance.name || '';
    return worldId && instanceId ? `${worldId}:${instanceId}` : '';
}

export function mergeGroupInstances(
    baseInstances: GroupInstanceRecord[],
    {
        groupId,
        friendsById,
        currentUserSnapshot,
        currentLocation
    }: MergeGroupInstancesOptions
): GroupDialogInstanceRow[] {
    const normalizedGroupId = groupId.trim();
    const currentLocationKey = normalizeLocation(currentLocation);
    const byLocation = new Map<string, GroupDialogInstanceRow>();

    function ensureInstance(
        location: unknown,
        seed: GroupInstanceRecord = {}
    ): GroupDialogInstanceRow | null {
        const normalizedLocation = normalizeLocation(location);
        if (!normalizedLocation) {
            return null;
        }
        const parsed = parseLocation(normalizedLocation);
        const embeddedInstance = entityRecord(seed.instance);
        const world =
            entityRecord(seed.world) || entityRecord(embeddedInstance?.world);
        const existing = byLocation.get(normalizedLocation);
        if (existing) {
            // 按来源排名决定谁赢：`'vrchat'` 权威，其余只补空缺。
            // 排名相同（含都是 `'unknown'`）时回落到「后到者优先」。
            const seedWins =
                originRank(seed.origin) >= originRank(existing.origin);
            const winner: GroupInstanceRecord = seedWins ? seed : existing;
            const loser: GroupInstanceRecord = seedWins ? existing : seed;

            // 只认「给了值」的字段，好让输的一方把空缺补上。
            const winnerFields: Record<string, unknown> = {};
            for (const [key, value] of Object.entries(winner)) {
                if (value === undefined || value === null) {
                    continue;
                }
                // 纯空白不算「给了值」：一个 `'   '` 会把输家的真值（星遇给的
                // 地图名、房间标题）覆盖成看不见的东西。
                if (typeof value === 'string' && value.trim() === '') {
                    continue;
                }
                winnerFields[key] = value;
            }

            const merged = {
                ...loser,
                ...winnerFields
            } as GroupInstanceRecord;

            const mergedWorldId = normalizeString(
                merged.worldId || world?.id || parsed.worldId
            );
            const mergedInstanceId = normalizeString(
                merged.instanceId ||
                    merged.id ||
                    parsed.instanceId ||
                    existing.instanceId
            );
            // `users` / `friendCount` 取并、取大：好友是 `addUser` 之后才填的，
            // 这一行已有的可能已经攒了人，选一侧会丢数据。
            const users = mergeUsers(existing.users, entityRows(seed.users));
            const friendCount = Math.max(
                Number(existing.friendCount) || 0,
                Number(seed.friendCount) || 0
            );

            return Object.assign(existing, merged, {
                worldId: mergedWorldId,
                instanceId: mergedInstanceId,
                ref:
                    entityRecord(merged.ref) ||
                    entityRecord(winner.ref) ||
                    embeddedInstance ||
                    existing.ref,
                location: normalizedLocation,
                tag: normalizedLocation,
                users,
                friendCount,
                // 合并后这一行归排名更高的来源。同排名时保留已有的那个，
                // 免得「先到」被后到者悄悄改写成别的身份。
                //
                // 两边都没标来源时要落 `'unknown'`（和新建行走同一个口径），
                // 不能留 `undefined`——否则同一字段会出现两种「没有来源」的表示。
                origin: parseInstanceOrigin(
                    seedWins
                        ? (seed.origin ?? existing.origin)
                        : (existing.origin ?? seed.origin)
                )
            });
        }

        const instanceId = normalizeString(
            seed.instanceId ||
                embeddedInstance?.instanceId ||
                seed.id ||
                parsed.instanceId
        );
        // 显式的 `friendCount` 优先，`0` 也算——`||` 会把 `0` 当假值，
        // 于是「明确没有好友」会被 `userCount`（总人数）顶掉。
        // VRChat 的实例本来就没有这个字段，那边仍然回落到 `userCount`。
        const declaredFriendCount =
            typeof seed.friendCount === 'number'
                ? seed.friendCount
                : Number(seed.userCount || 0) || 0;
        const row: GroupDialogInstanceRow = {
            ...embeddedInstance,
            ...seed,
            id: instanceId || normalizedLocation,
            location: normalizedLocation,
            tag: normalizedLocation,
            worldId: normalizeString(
                seed.worldId || world?.id || parsed.worldId
            ),
            instanceId,
            users: entityRows(seed.users),
            friendCount: declaredFriendCount,
            ref: entityRecord(seed.ref) || embeddedInstance || seed,
            // 认不出的来源落进 `'unknown'`，不会静悄悄当成 VRChat。
            origin: parseInstanceOrigin(seed.origin)
        };
        byLocation.set(normalizedLocation, row);
        return row;
    }

    for (const instance of baseInstances) {
        ensureInstance(instanceLocation(instance), instance);
    }

    function addUser(user: InstanceUser | null | undefined, isFriend = false) {
        const location = userGroupLocation(user);
        if (!location || !user) {
            return;
        }
        const parsed = parseLocation(location);
        if (normalizedGroupId && parsed.groupId !== normalizedGroupId) {
            return;
        }
        const row = byLocation.get(location);
        const userId = normalizeString(user.id || user.userId);
        if (
            !row ||
            !userId ||
            row.users.some(
                (existing) =>
                    normalizeString(existing.id || existing.userId) === userId
            )
        ) {
            return;
        }
        row.users.push(user);
        if (isFriend) {
            row.friendCount = Math.max(row.friendCount || 0, row.users.length);
        }
    }

    Object.values(friendsById).forEach((friend) => addUser(friend, true));
    if (currentUserSnapshot) {
        addUser(currentUserSnapshot, false);
    }

    return Array.from(byLocation.values())
        .map((row) => ({
            ...row,
            friendCount: row.friendCount || row.users.length,
            users: [...row.users].sort((left, right) =>
                String(left.displayName || left.id || '').localeCompare(
                    String(right.displayName || right.id || '')
                )
            )
        }))
        .sort((left, right) => {
            if (currentLocationKey && left.location === currentLocationKey) {
                return -1;
            }
            if (currentLocationKey && right.location === currentLocationKey) {
                return 1;
            }
            return (
                (right.users.length || Number(right.ref.userCount) || 0) -
                (left.users.length || Number(left.ref.userCount) || 0)
            );
        });
}
