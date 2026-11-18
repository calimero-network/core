from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PublisherBindingResponse200")


@_attrs_define
class PublisherBindingResponse200:
    """
    Attributes:
        signer_id (str):
        cluster (str):
        generation (int):
        bound (bool):
    """

    signer_id: str
    cluster: str
    generation: int
    bound: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        signer_id = self.signer_id

        cluster = self.cluster

        generation = self.generation

        bound = self.bound

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "signerId": signer_id,
                "cluster": cluster,
                "generation": generation,
                "bound": bound,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        signer_id = d.pop("signerId")

        cluster = d.pop("cluster")

        generation = d.pop("generation")

        bound = d.pop("bound")

        publisher_binding_response_200 = cls(
            signer_id=signer_id,
            cluster=cluster,
            generation=generation,
            bound=bound,
        )

        publisher_binding_response_200.additional_properties = d
        return publisher_binding_response_200

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
