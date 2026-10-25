import { type UpstreamConfig } from "ndscm/config/UPSTREAM"

export default {
  theseed: {
    converge: "melt",
    scm: "git",
    repo: "git@github.com:ndscm/theseed.git",
    tracking: "main",
    include: [
      // sort
      "^Cargo.lock$",
      "^seed/devprod/xslash/.*$",
    ],
    rename: [
      {
        match: "^seed/devprod/xslash/",
        replace: "",
      },
    ],
  },
} satisfies UpstreamConfig
