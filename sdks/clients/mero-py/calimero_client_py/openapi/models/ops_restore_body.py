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
    """

    backup: Union[Unset, str] = UNSET
    recovery: Union[Unset, "OpsRestoreBodyRecovery"] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof
        from ..models.ops_restore_body_recovery import OpsRestoreBodyRecovery

        backup = self.backup

        recovery: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.recovery, Unset):
            recovery = self.recovery.to_dict()

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if backup is not UNSET:
            field_dict["backup"] = backup
        if recovery is not UNSET:
            field_dict["recovery"] = recovery
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof
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

        ops_restore_body = cls(
            backup=backup,
            recovery=recovery,
            operator_proof=operator_proof,
        )

        return ops_restore_body
