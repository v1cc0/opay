import { prisma } from '@/lib/db';

// 内存缓存：key → { value, expiresAt }
const cache = new Map<string, { value: string; expiresAt: number }>();
const CACHE_TTL_MS = 30_000; // 30 秒

function getCached(key: string): string | undefined {
  const entry = cache.get(key);
  if (!entry) return undefined;
  if (Date.now() > entry.expiresAt) {
    cache.delete(key);
    return undefined;
  }
  return entry.value;
}

function setCache(key: string, value: string): void {
  cache.set(key, { value, expiresAt: Date.now() + CACHE_TTL_MS });
}

export function invalidateConfigCache(key?: string): void {
  if (key) {
    cache.delete(key);
  } else {
    cache.clear();
  }
}

export async function getSystemConfig(key: string): Promise<string | undefined> {
  const cached = getCached(key);
  if (cached !== undefined) return cached;

  const row = await prisma.systemConfig.findUnique({ where: { key } });
  if (row) {
    setCache(key, row.value);
    return row.value;
  }

  // 回退到环境变量
  const envVal = process.env[key];
  if (envVal !== undefined) {
    setCache(key, envVal);
  }
  return envVal;
}

export async function getSystemConfigs(keys: string[]): Promise<Record<string, string>> {
  const result: Record<string, string> = {};
  const missing: string[] = [];

  for (const key of keys) {
    const cached = getCached(key);
    if (cached !== undefined) {
      result[key] = cached;
    } else {
      missing.push(key);
    }
  }

  if (missing.length > 0) {
    const rows = await prisma.systemConfig.findMany({
      where: { key: { in: missing } },
    });

    const dbMap = new Map(rows.map((r) => [r.key, r.value]));

    for (const key of missing) {
      const val = dbMap.get(key) ?? process.env[key];
      if (val !== undefined) {
        result[key] = val;
        setCache(key, val);
      }
    }
  }

  return result;
}

export async function setSystemConfig(key: string, value: string, group?: string, label?: string): Promise<void> {
  await prisma.systemConfig.upsert({
    where: { key },
    update: { value, ...(group !== undefined && { group }), ...(label !== undefined && { label }) },
    create: { key, value, group: group ?? 'general', label },
  });
  invalidateConfigCache(key);
}

export async function setSystemConfigs(
  configs: { key: string; value: string; group?: string; label?: string }[],
): Promise<void> {
  await prisma.$transaction(
    configs.map((c) =>
      prisma.systemConfig.upsert({
        where: { key: c.key },
        update: {
          value: c.value,
          ...(c.group !== undefined && { group: c.group }),
          ...(c.label !== undefined && { label: c.label }),
        },
        create: { key: c.key, value: c.value, group: c.group ?? 'general', label: c.label },
      }),
    ),
  );
  invalidateConfigCache();
}

export async function getSystemConfigsByGroup(
  group: string,
): Promise<{ key: string; value: string; label: string | null }[]> {
  return prisma.systemConfig.findMany({
    where: { group },
    select: { key: true, value: true, label: true },
    orderBy: { key: 'asc' },
  });
}

export async function getAllSystemConfigs(): Promise<
  { key: string; value: string; group: string; label: string | null }[]
> {
  return prisma.systemConfig.findMany({
    select: { key: true, value: true, group: true, label: true },
    orderBy: [{ group: 'asc' }, { key: 'asc' }],
  });
}

export async function deleteSystemConfig(key: string): Promise<void> {
  await prisma.systemConfig.delete({ where: { key } }).catch(() => {});
  invalidateConfigCache(key);
}
