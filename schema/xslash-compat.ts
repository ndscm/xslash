/**
 * The actual xslash parser used for dotslash compatibility.
 */
export type XslashCompat = {
  name: string
  final?: boolean
  platforms?: {
    [platform: string]:
      | {
          hash: string
          digest: string
          size: number
          format?: string
          readonly?: boolean
          rw?: boolean
          path: string
          providers?: {
            type?: string
            url?: string
            [key: string]: unknown
          }[]
        }
      | {
          rw?: boolean
          path: string
          compose?: {
            hash: string
            digest: string
            size: number
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
