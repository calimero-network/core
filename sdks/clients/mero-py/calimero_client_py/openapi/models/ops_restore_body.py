from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.operator_proof import OperatorProof
    from ..models.restore_consent import RestoreConsent
    from ..models.ops_restore_body_recovery import OpsRestoreBodyRecovery


T = TypeVar("T", bound="OpsRestoreBody")


@_attrs_define
class OpsRestoreBody:
    """
    Attributes:
        backup (Union[Unset, str]):
        recovery (Union[Unset, OpsRestoreBodyRecovery]):
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
        owner (Union[Unset, RestoreConsent]): An owner's or admin's step-up over `<app> / __restore_older / { backup,
            sha256, bundle, point }` (`POST /apps/{app}/restore-consent`; `point` is the restore point its open undo would
            revert, by its time, target and snapshot): the operator's restore carries it as `owner`, with the operator's
            signature naming the same bundle, to take a pinned instance (its record's `lineage`) from that backup onto that
            bundle, older than the release it runs, while that undo is open at a time no earlier than the upgrade and any
            upgrade logged since. Spent once.
    """

    backup: Union[Unset, str] = UNSET
    recovery: Union[Unset, "OpsRestoreBodyRecovery"] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET
    owner: Union[Unset, "RestoreConsent"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof
        from ..models.restore_consent import RestoreConsent
        from ..models.ops_restore_body_recovery import OpsRestoreBodyRecovery

        backup = self.backup

        recovery: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.recovery, Unset):
            recovery = self.recovery.to_dict()

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        owner: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.owner, Unset):
            owner = self.owner.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if backup is not UNSET:
            field_dict["backup"] = backup
        if recovery is not UNSET:
            field_dict["recovery"] = recovery
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof
        if owner is not UNSET:
            field_dict["owner"] = owner

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof
        from ..models.restore_consent import RestoreConsent
        from ..models.ops_restore_body_recovery import OpsRestoreBodyRecovery

        d = dict(src_dict)
        backup = d.pop("backup", UNSET)

        _recovery = d.pop("recovery", UNSET)
        recovery: Union[Unset, OpsRestoreBodyRecovery]
        if isinstance(_recovery, Unset):
            recovery = UNSET
        else:
            recovery = OpsRestoreBodyRecovery.from_dict(_recovery)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        _owner = d.pop("owner", UNSET)
        owner: Union[Unset, RestoreConsent]
        if isinstance(_owner, Unset):
            owner = UNSET
        else:
            owner = RestoreConsent.from_dict(_owner)

        ops_restore_body = cls(
            backup=backup,
            recovery=recovery,
            operator_proof=operator_proof,
            owner=owner,
        )

        return ops_restore_body
