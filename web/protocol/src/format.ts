const UNITS = ['B', 'KiB', 'MiB', 'GiB'] as const

/** `912 B`, `4.2 KiB`, `13 MiB`: one decimal only while it still says something. */
export const formatBytes = (count: number): string => {
  let value = count
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  const digits = unit === 0 || value >= 10 ? 0 : 1
  return `${value.toFixed(digits)} ${UNITS[unit]}`
}
