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
    from ..models.confirm import Confirm
    from ..models.deployer_type_1 import DeployerType1
    from ..models.operator_proof import OperatorProof


T = TypeVar("T", bound="OpsDeployerGrantBody")


@_attrs_define
class OpsDeployerGrantBody:
    """
    Attributes:
        deployer (Union['DeployerType0', 'DeployerType1']): An account or an organization.
        until (Union[Unset, str]):
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    deployer: Union["DeployerType0", "DeployerType1"]
    until: Union[Unset, str] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.confirm import Confirm
        from ..models.deployer_type_1 import DeployerType1
        from ..models.operator_proof import OperatorProof

        deployer: dict[str, Any]
        if isinstance(self.deployer, DeployerType0):
            deployer = self.deployer.to_dict()
        else:
            deployer = self.deployer.to_dict()

        until = self.until

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "deployer": deployer,
            }
        )
        if until is not UNSET:
            field_dict["until"] = until
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.confirm import Confirm
        from ..models.deployer_type_1 import DeployerType1
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)

        def _parse_deployer(data: object) -> Union["DeployerType0", "DeployerType1"]:
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

        deployer = _parse_deployer(d.pop("deployer"))

        until = d.pop("until", UNSET)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        ops_deployer_grant_body = cls(
            deployer=deployer,
            until=until,
            operator_proof=operator_proof,
            confirm=confirm,
        )

        return ops_deployer_grant_body
