from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

T = TypeVar("T", bound="AppCreateResponse200")


@_attrs_define
class AppCreateResponse200:
    """
    Attributes:
        app_id (str):
        context_id (str):
        group (int):
        replicas (list[int]):
        compute (bool):
    """

    app_id: str
    context_id: str
    group: int
    replicas: list[int]
    compute: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        app_id = self.app_id

        context_id = self.context_id

        group = self.group

        replicas = self.replicas

        compute = self.compute

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "contextId": context_id,
                "group": group,
                "replicas": replicas,
                "compute": compute,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        app_id = d.pop("appId")

        context_id = d.pop("contextId")

        group = d.pop("group")

        replicas = cast(list[int], d.pop("replicas"))

        compute = d.pop("compute")

        app_create_response_200 = cls(
            app_id=app_id,
            context_id=context_id,
            group=group,
            replicas=replicas,
            compute=compute,
        )

        app_create_response_200.additional_properties = d
        return app_create_response_200

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
