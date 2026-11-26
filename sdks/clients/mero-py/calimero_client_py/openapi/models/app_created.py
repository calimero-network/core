from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.app_created_links import AppCreatedLinks


T = TypeVar("T", bound="AppCreated")


@_attrs_define
class AppCreated:
    """What `POST /apps` answers: the new instance, its app group and replicas, whether its release ships a compute
    component, and the apps it created with it by name.

        Attributes:
            app_id (str):
            group (int):
            replicas (list[int]):
            compute (bool):
            links (AppCreatedLinks):
    """

    app_id: str
    group: int
    replicas: list[int]
    compute: bool
    links: "AppCreatedLinks"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_created_links import AppCreatedLinks

        app_id = self.app_id

        group = self.group

        replicas = self.replicas

        compute = self.compute

        links = self.links.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "group": group,
                "replicas": replicas,
                "compute": compute,
                "links": links,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_created_links import AppCreatedLinks

        d = dict(src_dict)
        app_id = d.pop("appId")

        group = d.pop("group")

        replicas = cast(list[int], d.pop("replicas"))

        compute = d.pop("compute")

        links = AppCreatedLinks.from_dict(d.pop("links"))

        app_created = cls(
            app_id=app_id,
            group=group,
            replicas=replicas,
            compute=compute,
            links=links,
        )

        app_created.additional_properties = d
        return app_created

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
