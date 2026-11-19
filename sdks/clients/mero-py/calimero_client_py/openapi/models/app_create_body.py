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
    from ..models.grant_type_0 import GrantType0
    from ..models.grant_type_3 import GrantType3
    from ..models.grant_type_1 import GrantType1
    from ..models.access_type_0 import AccessType0
    from ..models.app_create_body_new_org import AppCreateBodyNewOrg
    from ..models.grant_type_2 import GrantType2
    from ..models.access_type_3 import AccessType3
    from ..models.access_type_1 import AccessType1
    from ..models.access_type_4 import AccessType4
    from ..models.access_type_2 import AccessType2
    from ..models.grant_type_4 import GrantType4
    from ..models.access_type_5 import AccessType5


T = TypeVar("T", bound="AppCreateBody")


@_attrs_define
class AppCreateBody:
    """
    Attributes:
        name (Union[Unset, str]):
        bundle (Union[Unset, str]):
        package (Union[Unset, str]):
        version (Union[Unset, str]):
        service (Union[Unset, str]):
        org (Union[Unset, str]):
        new_org (Union[Unset, AppCreateBodyNewOrg]):
        access (Union['AccessType0', 'AccessType1', 'AccessType2', 'AccessType3', 'AccessType4', 'AccessType5', Unset]):
            Who may reach an app at all, as one mode (deprecated: see `grants`), or `grants`: the instance's grant list (see
            #Grant). Each mode is a grant list: `owner` none, `org` the organization, `team` that team, `accounts` those
            people, `public` `anyone`.
        grants (Union[Unset, list[Union['GrantType0', 'GrantType1', 'GrantType2', 'GrantType3', 'GrantType4']]]):
        init_args (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for
            whose it is).
        metadata (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for
            whose it is).
        deployment (Union[Unset, str]):
    """

    name: Union[Unset, str] = UNSET
    bundle: Union[Unset, str] = UNSET
    package: Union[Unset, str] = UNSET
    version: Union[Unset, str] = UNSET
    service: Union[Unset, str] = UNSET
    org: Union[Unset, str] = UNSET
    new_org: Union[Unset, "AppCreateBodyNewOrg"] = UNSET
    access: Union[
        "AccessType0",
        "AccessType1",
        "AccessType2",
        "AccessType3",
        "AccessType4",
        "AccessType5",
        Unset,
    ] = UNSET
    grants: Union[
        Unset,
        list[
            Union["GrantType0", "GrantType1", "GrantType2", "GrantType3", "GrantType4"]
        ],
    ] = UNSET
    init_args: Union[Unset, Any] = UNSET
    metadata: Union[Unset, Any] = UNSET
    deployment: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.grant_type_0 import GrantType0
        from ..models.grant_type_3 import GrantType3
        from ..models.grant_type_1 import GrantType1
        from ..models.access_type_0 import AccessType0
        from ..models.app_create_body_new_org import AppCreateBodyNewOrg
        from ..models.grant_type_2 import GrantType2
        from ..models.access_type_3 import AccessType3
        from ..models.access_type_1 import AccessType1
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.grant_type_4 import GrantType4
        from ..models.access_type_5 import AccessType5

        name = self.name

        bundle = self.bundle

        package = self.package

        version = self.version

        service = self.service

        org = self.org

        new_org: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.new_org, Unset):
            new_org = self.new_org.to_dict()

        access: Union[Unset, dict[str, Any]]
        if isinstance(self.access, Unset):
            access = UNSET
        elif isinstance(self.access, AccessType0):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType1):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType2):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType3):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType4):
            access = self.access.to_dict()
        else:
            access = self.access.to_dict()

        grants: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.grants, Unset):
            grants = []
            for grants_item_data in self.grants:
                grants_item: dict[str, Any]
                if isinstance(grants_item_data, GrantType0):
                    grants_item = grants_item_data.to_dict()
                elif isinstance(grants_item_data, GrantType1):
                    grants_item = grants_item_data.to_dict()
                elif isinstance(grants_item_data, GrantType2):
                    grants_item = grants_item_data.to_dict()
                elif isinstance(grants_item_data, GrantType3):
                    grants_item = grants_item_data.to_dict()
                else:
                    grants_item = grants_item_data.to_dict()

                grants.append(grants_item)

        init_args = self.init_args

        metadata = self.metadata

        deployment = self.deployment

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if name is not UNSET:
            field_dict["name"] = name
        if bundle is not UNSET:
            field_dict["bundle"] = bundle
        if package is not UNSET:
            field_dict["package"] = package
        if version is not UNSET:
            field_dict["version"] = version
        if service is not UNSET:
            field_dict["service"] = service
        if org is not UNSET:
            field_dict["org"] = org
        if new_org is not UNSET:
            field_dict["newOrg"] = new_org
        if access is not UNSET:
            field_dict["access"] = access
        if grants is not UNSET:
            field_dict["grants"] = grants
        if init_args is not UNSET:
            field_dict["initArgs"] = init_args
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if deployment is not UNSET:
            field_dict["deployment"] = deployment

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.grant_type_0 import GrantType0
        from ..models.grant_type_3 import GrantType3
        from ..models.grant_type_1 import GrantType1
        from ..models.access_type_0 import AccessType0
        from ..models.app_create_body_new_org import AppCreateBodyNewOrg
        from ..models.grant_type_2 import GrantType2
        from ..models.access_type_3 import AccessType3
        from ..models.access_type_1 import AccessType1
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.grant_type_4 import GrantType4
        from ..models.access_type_5 import AccessType5

        d = dict(src_dict)
        name = d.pop("name", UNSET)

        bundle = d.pop("bundle", UNSET)

        package = d.pop("package", UNSET)

        version = d.pop("version", UNSET)

        service = d.pop("service", UNSET)

        org = d.pop("org", UNSET)

        _new_org = d.pop("newOrg", UNSET)
        new_org: Union[Unset, AppCreateBodyNewOrg]
        if isinstance(_new_org, Unset):
            new_org = UNSET
        else:
            new_org = AppCreateBodyNewOrg.from_dict(_new_org)

        def _parse_access(
            data: object,
        ) -> Union[
            "AccessType0",
            "AccessType1",
            "AccessType2",
            "AccessType3",
            "AccessType4",
            "AccessType5",
            Unset,
        ]:
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_0 = AccessType0.from_dict(data)

                return componentsschemas_access_type_0
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_1 = AccessType1.from_dict(data)

                return componentsschemas_access_type_1
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_2 = AccessType2.from_dict(data)

                return componentsschemas_access_type_2
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_3 = AccessType3.from_dict(data)

                return componentsschemas_access_type_3
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_4 = AccessType4.from_dict(data)

                return componentsschemas_access_type_4
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_access_type_5 = AccessType5.from_dict(data)

            return componentsschemas_access_type_5

        access = _parse_access(d.pop("access", UNSET))

        grants = []
        _grants = d.pop("grants", UNSET)
        for grants_item_data in _grants or []:

            def _parse_grants_item(
                data: object,
            ) -> Union[
                "GrantType0", "GrantType1", "GrantType2", "GrantType3", "GrantType4"
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_grant_type_0 = GrantType0.from_dict(data)

                    return componentsschemas_grant_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_grant_type_1 = GrantType1.from_dict(data)

                    return componentsschemas_grant_type_1
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_grant_type_2 = GrantType2.from_dict(data)

                    return componentsschemas_grant_type_2
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_grant_type_3 = GrantType3.from_dict(data)

                    return componentsschemas_grant_type_3
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_grant_type_4 = GrantType4.from_dict(data)

                return componentsschemas_grant_type_4

            grants_item = _parse_grants_item(grants_item_data)

            grants.append(grants_item)

        init_args = d.pop("initArgs", UNSET)

        metadata = d.pop("metadata", UNSET)

        deployment = d.pop("deployment", UNSET)

        app_create_body = cls(
            name=name,
            bundle=bundle,
            package=package,
            version=version,
            service=service,
            org=org,
            new_org=new_org,
            access=access,
            grants=grants,
            init_args=init_args,
            metadata=metadata,
            deployment=deployment,
        )

        app_create_body.additional_properties = d
        return app_create_body

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
