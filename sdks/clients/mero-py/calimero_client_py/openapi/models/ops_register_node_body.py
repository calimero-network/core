from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.ops_register_node_body_role import OpsRegisterNodeBodyRole
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.operator_proof import OperatorProof


T = TypeVar("T", bound="OpsRegisterNodeBody")


@_attrs_define
class OpsRegisterNodeBody:
    """
    Attributes:
        id (int):
        addr (str):
        zone (Union[Unset, str]):
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
        role (Union[Unset, OpsRegisterNodeBodyRole]):
        pool (Union[Unset, str]):
    """

    id: int
    addr: str
    zone: Union[Unset, str] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET
    role: Union[Unset, OpsRegisterNodeBodyRole] = UNSET
    pool: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof

        id = self.id

        addr = self.addr

        zone = self.zone

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        role: Union[Unset, str] = UNSET
        if not isinstance(self.role, Unset):
            role = self.role.value

        pool = self.pool

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "addr": addr,
            }
        )
        if zone is not UNSET:
            field_dict["zone"] = zone
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof
        if role is not UNSET:
            field_dict["role"] = role
        if pool is not UNSET:
            field_dict["pool"] = pool

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)
        id = d.pop("id")

        addr = d.pop("addr")

        zone = d.pop("zone", UNSET)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        _role = d.pop("role", UNSET)
        role: Union[Unset, OpsRegisterNodeBodyRole]
        if isinstance(_role, Unset):
            role = UNSET
        else:
            role = OpsRegisterNodeBodyRole(_role)

        pool = d.pop("pool", UNSET)

        ops_register_node_body = cls(
            id=id,
            addr=addr,
            zone=zone,
            operator_proof=operator_proof,
            role=role,
            pool=pool,
        )

        ops_register_node_body.additional_properties = d
        return ops_register_node_body

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
