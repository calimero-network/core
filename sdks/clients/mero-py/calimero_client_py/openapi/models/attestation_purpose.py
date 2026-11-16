from enum import Enum


class AttestationPurpose(str, Enum):
    KMS = "kms"
    ORCHESTRATOR = "orchestrator"
    RECEIPTS = "receipts"

    def __str__(self) -> str:
        return str(self.value)
