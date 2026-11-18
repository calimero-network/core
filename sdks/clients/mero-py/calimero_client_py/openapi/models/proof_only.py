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


T = TypeVar("T", bound="ProofOnly")


@_attrs_define
class ProofOnly:
    """What an operator decision that names nothing in its body may carry: none at first, the operator's proof after the
    `428`.

        Attributes:
            operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
                `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
    """

    operator_proof: Union[Unset, "OperatorProof"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)
        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        proof_only = cls(
            operator_proof=operator_proof,
        )

        return proof_only
