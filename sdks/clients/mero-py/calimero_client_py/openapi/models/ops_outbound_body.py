from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.outbound_selection_type_0 import OutboundSelectionType0
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.operator_proof import OperatorProof


T = TypeVar("T", bound="OpsOutboundBody")


@_attrs_define
class OpsOutboundBody:
    """
    Attributes:
        package (str):
        prefixes (Union[OutboundSelectionType0, Unset, list[str]]): `manifest` (every prefix the release declares) or
            these prefixes.
        replace (Union[Unset, bool]):
        operator_proof (Union[Unset, OperatorProof]): The operator key's Ed25519 signature (hex) over the statement a
            `428 OPERATOR_SIGNATURE_REQUIRED` named, with that statement's nonce.
    """

    package: str
    prefixes: Union[OutboundSelectionType0, Unset, list[str]] = UNSET
    replace: Union[Unset, bool] = UNSET
    operator_proof: Union[Unset, "OperatorProof"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.operator_proof import OperatorProof

        package = self.package

        prefixes: Union[Unset, list[str], str]
        if isinstance(self.prefixes, Unset):
            prefixes = UNSET
        elif isinstance(self.prefixes, OutboundSelectionType0):
            prefixes = self.prefixes.value
        else:
            prefixes = self.prefixes

        replace = self.replace

        operator_proof: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.operator_proof, Unset):
            operator_proof = self.operator_proof.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
            }
        )
        if prefixes is not UNSET:
            field_dict["prefixes"] = prefixes
        if replace is not UNSET:
            field_dict["replace"] = replace
        if operator_proof is not UNSET:
            field_dict["operatorProof"] = operator_proof

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.operator_proof import OperatorProof

        d = dict(src_dict)
        package = d.pop("package")

        def _parse_prefixes(
            data: object,
        ) -> Union[OutboundSelectionType0, Unset, list[str]]:
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                componentsschemas_outbound_selection_type_0 = OutboundSelectionType0(
                    data
                )

                return componentsschemas_outbound_selection_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, list):
                raise TypeError()
            componentsschemas_outbound_selection_type_1 = cast(list[str], data)

            return componentsschemas_outbound_selection_type_1

        prefixes = _parse_prefixes(d.pop("prefixes", UNSET))

        replace = d.pop("replace", UNSET)

        _operator_proof = d.pop("operatorProof", UNSET)
        operator_proof: Union[Unset, OperatorProof]
        if isinstance(_operator_proof, Unset):
            operator_proof = UNSET
        else:
            operator_proof = OperatorProof.from_dict(_operator_proof)

        ops_outbound_body = cls(
            package=package,
            prefixes=prefixes,
            replace=replace,
            operator_proof=operator_proof,
        )

        ops_outbound_body.additional_properties = d
        return ops_outbound_body

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
