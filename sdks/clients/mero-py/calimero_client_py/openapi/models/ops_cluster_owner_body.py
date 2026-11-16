from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.deployer_type_0 import DeployerType0
    from ..models.deployer_type_1 import DeployerType1
    from ..models.operator_proof import OperatorProof


T = TypeVar("T", bound="OpsClusterOwnerBody")


@_attrs_define
class OpsClusterOwnerBody:
    """
    Attributes:
        owner (Union['DeployerType0', 'DeployerType1']): An account or an organization.
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
    """

    owner: Union["DeployerType0", "DeployerType1"]
    operator_proof: Union[Unset, "OperatorProof"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1
        from ..models.operator_proof import OperatorProof

        owner: dict[str, Any]
        if isinstance(self.owner, DeployerType0):
            owner = self.owner.to_dict()
        else:
            owner = self.owner.to_dict()

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "owner": owner,
            }
        )
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)

        def _parse_owner(data: object) -> Union["DeployerType0", "DeployerType1"]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_deployer_type_0 = DeployerType0.from_dict(data)

                return componentsschemas_deployer_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_deployer_type_1 = DeployerType1.from_dict(data)

            return componentsschemas_deployer_type_1

        owner = _parse_owner(d.pop("owner"))

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        ops_cluster_owner_body = cls(
            owner=owner,
            operator_proof=operator_proof,
        )

        return ops_cluster_owner_body
