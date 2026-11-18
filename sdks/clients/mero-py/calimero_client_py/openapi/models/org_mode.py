from enum import Enum


class OrgMode(str, Enum):
    LEGACY = "legacy"
    SHARED = "shared"
    STANDALONE = "standalone"

    def __str__(self) -> str:
        return str(self.value)
