from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_get_response_200_components import AppGetResponse200Components
from ..models.app_get_response_200_executor import AppGetResponse200Executor
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.grant_type_0 import GrantType0
    from ..models.grant_type_3 import GrantType3
    from ..models.grant_type_1 import GrantType1
    from ..models.app_get_response_200_restore import AppGetResponse200Restore
    from ..models.access_type_0 import AccessType0
    from ..models.grant_type_2 import GrantType2
    from ..models.access_type_3 import AccessType3
    from ..models.app_get_response_200_attached import AppGetResponse200Attached
    from ..models.access_type_1 import AccessType1
    from ..models.package_ref import PackageRef
    from ..models.access_type_4 import AccessType4
    from ..models.access_type_2 import AccessType2
    from ..models.app_get_response_200_links import AppGetResponse200Links
    from ..models.upgrade import Upgrade
    from ..models.access_type_5 import AccessType5
    from ..models.grant_type_4 import GrantType4


T = TypeVar("T", bound="AppGetResponse200")


@_attrs_define
class AppGetResponse200:
    """
    Attributes:
        app_id (str):
        name (str):
        group (int):
        bundle (str):
        owner (str):
        org (Union[None, str]):
        access (Union['AccessType0', 'AccessType1', 'AccessType2', 'AccessType3', 'AccessType4', 'AccessType5']): Who
            may reach an app at all, as one mode (deprecated: see `grants`), or `grants`: the instance's grant list (see
            #Grant). Each mode is a grant list: `owner` none, `org` the organization, `team` that team, `accounts` those
            people, `public` `anyone`.
        confirm_methods (list[str]):
        replicas (list[int]):
        created_at (int):
        init_args (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
        metadata (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
        executor (Union[Unset, AppGetResponse200Executor]):
        compute_bundle (Union[Unset, str]):
        components (Union[Unset, AppGetResponse200Components]):
        last_upgrade (Union[Unset, Upgrade]): The last upgrade tried on an app, done or failed: `from` and `to` are
            bundle hashes, `by` the account (or `operator`), `run` the organization-wide run it was part of, `apiKey` the
            upgrade key that made it.
        deployment (Union[Unset, str]):
        release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-service
            bundle, `compute` (absent: false) marks the service's compute component.
        restore (Union[Unset, AppGetResponse200Restore]):
        links (Union[Unset, AppGetResponse200Links]):
        attached (Union[Unset, AppGetResponse200Attached]):
        grants (Union[Unset, list[Union['GrantType0', 'GrantType1', 'GrantType2', 'GrantType3', 'GrantType4']]]):
    """

    app_id: str
    name: str
    group: int
    bundle: str
    owner: str
    org: Union[None, str]
    access: Union[
        "AccessType0",
        "AccessType1",
        "AccessType2",
        "AccessType3",
        "AccessType4",
        "AccessType5",
    ]
    confirm_methods: list[str]
    replicas: list[int]
    created_at: int
    init_args: Any
    metadata: Any
    executor: Union[Unset, AppGetResponse200Executor] = UNSET
    compute_bundle: Union[Unset, str] = UNSET
    components: Union[Unset, AppGetResponse200Components] = UNSET
    last_upgrade: Union[Unset, "Upgrade"] = UNSET
    deployment: Union[Unset, str] = UNSET
    release: Union[Unset, "PackageRef"] = UNSET
    restore: Union[Unset, "AppGetResponse200Restore"] = UNSET
    links: Union[Unset, "AppGetResponse200Links"] = UNSET
    attached: Union[Unset, "AppGetResponse200Attached"] = UNSET
    grants: Union[
        Unset,
        list[
            Union["GrantType0", "GrantType1", "GrantType2", "GrantType3", "GrantType4"]
        ],
    ] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.grant_type_0 import GrantType0
        from ..models.grant_type_3 import GrantType3
        from ..models.grant_type_1 import GrantType1
        from ..models.app_get_response_200_restore import AppGetResponse200Restore
        from ..models.access_type_0 import AccessType0
        from ..models.grant_type_2 import GrantType2
        from ..models.access_type_3 import AccessType3
        from ..models.app_get_response_200_attached import AppGetResponse200Attached
        from ..models.access_type_1 import AccessType1
        from ..models.package_ref import PackageRef
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.app_get_response_200_links import AppGetResponse200Links
        from ..models.upgrade import Upgrade
        from ..models.access_type_5 import AccessType5
        from ..models.grant_type_4 import GrantType4

        app_id = self.app_id

        name = self.name

        group = self.group

        bundle = self.bundle

        owner = self.owner

        org: Union[None, str]
        org = self.org

        access: dict[str, Any]
        if isinstance(self.access, AccessType0):
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

        confirm_methods = self.confirm_methods

        replicas = self.replicas

        created_at = self.created_at

        init_args = self.init_args

        metadata = self.metadata

        executor: Union[Unset, str] = UNSET
        if not isinstance(self.executor, Unset):
            executor = self.executor.value

        compute_bundle = self.compute_bundle

        components: Union[Unset, str] = UNSET
        if not isinstance(self.components, Unset):
            components = self.components.value

        last_upgrade: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.last_upgrade, Unset):
            last_upgrade = self.last_upgrade.to_dict()

        deployment = self.deployment

        release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.release, Unset):
            release = self.release.to_dict()

        restore: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.restore, Unset):
            restore = self.restore.to_dict()

        links: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.links, Unset):
            links = self.links.to_dict()

        attached: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.attached, Unset):
            attached = self.attached.to_dict()

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

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "name": name,
                "group": group,
                "bundle": bundle,
                "owner": owner,
                "org": org,
                "access": access,
                "confirmMethods": confirm_methods,
                "replicas": replicas,
                "createdAt": created_at,
                "initArgs": init_args,
                "metadata": metadata,
            }
        )
        if executor is not UNSET:
            field_dict["executor"] = executor
        if compute_bundle is not UNSET:
            field_dict["computeBundle"] = compute_bundle
        if components is not UNSET:
            field_dict["components"] = components
        if last_upgrade is not UNSET:
            field_dict["lastUpgrade"] = last_upgrade
        if deployment is not UNSET:
            field_dict["deployment"] = deployment
        if release is not UNSET:
            field_dict["release"] = release
        if restore is not UNSET:
            field_dict["restore"] = restore
        if links is not UNSET:
            field_dict["links"] = links
        if attached is not UNSET:
            field_dict["attached"] = attached
        if grants is not UNSET:
            field_dict["grants"] = grants

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.grant_type_0 import GrantType0
        from ..models.grant_type_3 import GrantType3
        from ..models.grant_type_1 import GrantType1
        from ..models.app_get_response_200_restore import AppGetResponse200Restore
        from ..models.access_type_0 import AccessType0
        from ..models.grant_type_2 import GrantType2
        from ..models.access_type_3 import AccessType3
        from ..models.app_get_response_200_attached import AppGetResponse200Attached
        from ..models.access_type_1 import AccessType1
        from ..models.package_ref import PackageRef
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.app_get_response_200_links import AppGetResponse200Links
        from ..models.upgrade import Upgrade
        from ..models.access_type_5 import AccessType5
        from ..models.grant_type_4 import GrantType4

        d = dict(src_dict)
        app_id = d.pop("appId")

        name = d.pop("name")

        group = d.pop("group")

        bundle = d.pop("bundle")

        owner = d.pop("owner")

        def _parse_org(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        org = _parse_org(d.pop("org"))

        def _parse_access(
            data: object,
        ) -> Union[
            "AccessType0",
            "AccessType1",
            "AccessType2",
            "AccessType3",
            "AccessType4",
            "AccessType5",
        ]:
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

        access = _parse_access(d.pop("access"))

        confirm_methods = cast(list[str], d.pop("confirmMethods"))

        replicas = cast(list[int], d.pop("replicas"))

        created_at = d.pop("createdAt")

        init_args = d.pop("initArgs")

        metadata = d.pop("metadata")

        _executor = d.pop("executor", UNSET)
        executor: Union[Unset, AppGetResponse200Executor]
        if isinstance(_executor, Unset):
            executor = UNSET
        else:
            executor = AppGetResponse200Executor(_executor)

        compute_bundle = d.pop("computeBundle", UNSET)

        _components = d.pop("components", UNSET)
        components: Union[Unset, AppGetResponse200Components]
        if isinstance(_components, Unset):
            components = UNSET
        else:
            components = AppGetResponse200Components(_components)

        _last_upgrade = d.pop("lastUpgrade", UNSET)
        last_upgrade: Union[Unset, Upgrade]
        if isinstance(_last_upgrade, Unset):
            last_upgrade = UNSET
        else:
            last_upgrade = Upgrade.from_dict(_last_upgrade)

        deployment = d.pop("deployment", UNSET)

        _release = d.pop("release", UNSET)
        release: Union[Unset, PackageRef]
        if isinstance(_release, Unset):
            release = UNSET
        else:
            release = PackageRef.from_dict(_release)

        _restore = d.pop("restore", UNSET)
        restore: Union[Unset, AppGetResponse200Restore]
        if isinstance(_restore, Unset):
            restore = UNSET
        else:
            restore = AppGetResponse200Restore.from_dict(_restore)

        _links = d.pop("links", UNSET)
        links: Union[Unset, AppGetResponse200Links]
        if isinstance(_links, Unset):
            links = UNSET
        else:
            links = AppGetResponse200Links.from_dict(_links)

        _attached = d.pop("attached", UNSET)
        attached: Union[Unset, AppGetResponse200Attached]
        if isinstance(_attached, Unset):
            attached = UNSET
        else:
            attached = AppGetResponse200Attached.from_dict(_attached)

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

        app_get_response_200 = cls(
            app_id=app_id,
            name=name,
            group=group,
            bundle=bundle,
            owner=owner,
            org=org,
            access=access,
            confirm_methods=confirm_methods,
            replicas=replicas,
            created_at=created_at,
            init_args=init_args,
            metadata=metadata,
            executor=executor,
            compute_bundle=compute_bundle,
            components=components,
            last_upgrade=last_upgrade,
            deployment=deployment,
            release=release,
            restore=restore,
            links=links,
            attached=attached,
            grants=grants,
        )

        app_get_response_200.additional_properties = d
        return app_get_response_200

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
