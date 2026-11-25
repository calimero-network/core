from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="ReceiptKey")


@_attrs_define
class ReceiptKey:
    """`GET /ops/nodes/{id}/receipt-key`: the receipt key this node holds for node `id`, the release it attested (`null`:
    unknown) and when.

        Attributes:
            node (int):
            key (str):
            release (Union[None, str]):
            attested_at (int):
    """

    node: int
    key: str
    release: Union[None, str]
    attested_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        node = self.node

        key = self.key

        release: Union[None, str]
        release = self.release

        attested_at = self.attested_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "node": node,
                "key": key,
                "release": release,
                "attestedAt": attested_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        node = d.pop("node")

        key = d.pop("key")

        def _parse_release(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        release = _parse_release(d.pop("release"))

        attested_at = d.pop("attestedAt")

        receipt_key = cls(
            node=node,
            key=key,
            release=release,
            attested_at=attested_at,
        )

        receipt_key.additional_properties = d
        return receipt_key

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
