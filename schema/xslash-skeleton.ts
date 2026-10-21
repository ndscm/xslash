/**
 * The suggested skeleton type for creating or bumping xslash entries.
 */
export type XslashSkeleton = {
  name: string
  platforms?: {
    [platform: string]:
      | {
          format?: string
          rw?: boolean
          path: string
          providers?: {
            url: string
          }[]
        }
      | {
          rw?: boolean
          path: string
          compose?: {
            format?: string
            strip?: string
            path?: string
            providers: {
              url: string
            }[]
          }[]
        }
  }
}
